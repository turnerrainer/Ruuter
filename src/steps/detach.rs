//! Issue #137 — `detach` step: continue DSL work after the HTTP
//! response is sent.
//!
//! The step composes with `parallel_http` (#135/#136) for the eFTI K4
//! pattern (kemit-ee/efti-gate-ee#252): fan out to peer gates in the
//! background, write the aggregated structured result array to
//! Postgres via Resql, caller polls back on a sibling route.
//!
//! Executor contract:
//!   1. Clone the parent context via `ExecutionContext::snapshot` —
//!      writes inside `do:` do not propagate back to the parent.
//!   2. Try to acquire a permit from the process-wide
//!      `DetachRegistry`'s Semaphore (sized by
//!      `AppConfig.detach.max_inflight`, default 256). Overflow fails
//!      the step with `RuuterError::DslExecution` — the caller can
//!      route to `error:` or fall through.
//!   3. Spawn a tokio task that runs `do:` sub-steps sequentially via
//!      `engine.execute_single_step`. The permit is held for the
//!      lifetime of the task.
//!   4. The task is registered in a process-wide `JoinSet<()>` so the
//!      SIGTERM drain in `main.rs` can wait for it to complete (up to
//!      `AppConfig.detach.shutdown_grace_secs`, default 15 s).
//!   5. Errors inside `do:` are logged with the parent's traceparent
//!      and never affect the parent's already-sent response.

use crate::context::ExecutionContext;
use crate::logging::error_chain;
use crate::steps::engine::StepEngine;
use crate::steps::{DetachStep, DslStep, StepExecutor, StepLogExtras, StepResult};
use crate::{Result, RuuterError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, TryAcquireError};
use tokio::task::JoinSet;

/// Process-wide detach-task bookkeeping. Shared by cloning (Arcs
/// inside). One instance per Ruuter process; `StepEngine` holds a
/// clone, `main.rs` retains one for the SIGTERM drain.
#[derive(Clone)]
pub struct DetachRegistry {
    /// Permit source. Bounds concurrent in-flight detached tasks
    /// across the whole process. `None` = unbounded (null-opt-out).
    semaphore: Option<Arc<Semaphore>>,
    cap: Option<u32>,
    /// Owns the detached tasks so the SIGTERM drain can wait for
    /// them. Guarded by a `tokio::sync::Mutex` because the executor
    /// (holds briefly to spawn) and the shutdown drain (holds for
    /// the whole drain window) are both async.
    tasks: Arc<Mutex<JoinSet<()>>>,
    /// Seconds the SIGTERM drain waits for `tasks` to finish before
    /// `abort_all()`. Mirrors `main.rs::SHUTDOWN_GRACE_SECS` for the
    /// HTTP serve path.
    shutdown_grace_secs: u64,
}

impl DetachRegistry {
    pub fn new(cfg: &crate::config::DetachConfig) -> Self {
        Self {
            semaphore: cfg
                .max_inflight
                .map(|n| Arc::new(Semaphore::new(n as usize))),
            cap: cfg.max_inflight,
            tasks: Arc::new(Mutex::new(JoinSet::new())),
            shutdown_grace_secs: cfg.shutdown_grace_secs,
        }
    }

    /// Semaphore capacity. `None` = unbounded. Used for structured
    /// error messages when overflow fires.
    pub fn cap(&self) -> Option<u32> {
        self.cap
    }

    /// SIGTERM-drain grace window in seconds.
    pub fn shutdown_grace_secs(&self) -> u64 {
        self.shutdown_grace_secs
    }

    /// Count of detached tasks the drain still has outstanding.
    /// Snapshot for ops diagnostics; may be racy under heavy churn.
    pub async fn inflight(&self) -> usize {
        self.tasks.lock().await.len()
    }

    /// Try to acquire a permit. Returns `Err` if the semaphore is
    /// full — caller surfaces as `RuuterError::DslExecution`. Returns
    /// `Ok(None)` when `max_inflight` is unconfigured (unbounded).
    fn try_acquire(&self) -> std::result::Result<Option<tokio::sync::OwnedSemaphorePermit>, ()> {
        match &self.semaphore {
            None => Ok(None),
            Some(sem) => match sem.clone().try_acquire_owned() {
                Ok(p) => Ok(Some(p)),
                Err(TryAcquireError::NoPermits) => Err(()),
                Err(TryAcquireError::Closed) => Err(()),
            },
        }
    }

    /// Spawn a detached task. The permit (if any) is moved into the
    /// task so it is released only when the task exits. Caller gets
    /// an error if overflow fires; the task is NOT spawned in that
    /// case.
    pub async fn try_spawn<F>(&self, fut: F) -> std::result::Result<(), DetachOverflow>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let permit = match self.try_acquire() {
            Ok(p) => p,
            Err(()) => {
                return Err(DetachOverflow {
                    cap: self.cap.unwrap_or(0),
                });
            }
        };
        let mut tasks = self.tasks.lock().await;
        tasks.spawn(async move {
            // Permit lives for the lifetime of the task; released on
            // normal completion, cancellation, or panic.
            let _permit = permit;
            fut.await;
        });
        Ok(())
    }

    /// SIGTERM drain. Waits for every detached task to finish, up to
    /// `shutdown_grace_secs`. Tasks still running past the window are
    /// `abort_all()`'d. Logs the outcome. Idempotent — callers that
    /// invoke twice see an immediate `(0, 0)`.
    pub async fn drain(&self) -> (usize, usize) {
        let mut tasks = self.tasks.lock().await;
        let starting = tasks.len();
        if starting == 0 {
            return (0, 0);
        }
        tracing::info!(
            inflight = starting,
            grace_secs = self.shutdown_grace_secs,
            "detach drain: waiting for inflight tasks"
        );
        let drain = async { while tasks.join_next().await.is_some() {} };
        let aborted = match tokio::time::timeout(
            Duration::from_secs(self.shutdown_grace_secs),
            drain,
        )
        .await
        {
            Ok(()) => 0,
            Err(_) => {
                let still = tasks.len();
                tracing::warn!(
                    inflight = still,
                    grace_secs = self.shutdown_grace_secs,
                    "detach drain: grace exceeded, aborting"
                );
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                still
            }
        };
        (starting, aborted)
    }
}

/// Signal returned by `try_spawn` when the Semaphore is full.
#[derive(Debug)]
pub struct DetachOverflow {
    pub cap: u32,
}

pub struct DetachStepExecutor {
    step: DetachStep,
    engine: StepEngine,
}

impl DetachStepExecutor {
    pub fn new(step: DetachStep, engine: StepEngine) -> Self {
        Self { step, engine }
    }
}

impl StepExecutor for DetachStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let body = &self.step.detach;

        if body.body.is_empty() {
            // Parse-time guarantees this doesn't happen on a loaded
            // DSL, but keep a defence here for programmatically-
            // constructed steps.
            return Err(RuuterError::InvalidStep(
                "detach.do must contain at least one step".into(),
            ));
        }

        let registry = match self.engine.detach_registry() {
            Some(r) => r.clone(),
            None => {
                return Err(RuuterError::InvalidStep(
                    "detach step used but no DetachRegistry wired on the engine \
                     (call StepEngine::with_detach_registry during boot)"
                        .into(),
                ));
            }
        };

        let snapshot = context.snapshot();
        let steps: Vec<DslStep> = body.body.clone();
        let timeout = body.timeout_ms.map(Duration::from_millis);
        let engine = self.engine.clone();
        let traceparent = snapshot.traceparent().map(String::from);
        let step_count = steps.len();

        let run = async move {
            let outcome = async {
                for sub in &steps {
                    match engine.execute_single_step(sub, &snapshot).await {
                        Ok(result) => {
                            // `return:` is a parse-time error for
                            // detach — a sub-step that reports
                            // should_return here is a defensive bail,
                            // same posture as iterate's early-exit.
                            if result.should_return {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %error_chain(&e),
                                traceparent = ?traceparent,
                                "detach: sub-step errored (parent response already sent)"
                            );
                            return;
                        }
                    }
                }
            };
            match timeout {
                Some(d) => {
                    if tokio::time::timeout(d, outcome).await.is_err() {
                        tracing::warn!(
                            traceparent = ?traceparent,
                            timeout_ms = d.as_millis() as u64,
                            "detach: timeout; remaining sub-steps cancelled"
                        );
                    }
                }
                None => outcome.await,
            }
        };

        match registry.try_spawn(run).await {
            Ok(()) => {
                let extras = StepLogExtras::new()
                    .push("steps", step_count as u64)
                    .push("detached", true);
                Ok(StepResult {
                    next_step: self.step.next.clone(),
                    log_extras: extras,
                    ..StepResult::new()
                })
            }
            Err(DetachOverflow { cap }) => Err(RuuterError::DslExecution {
                step: "detach".into(),
                message: format!(
                    "detach registry full (cap: {}). Reduce fan-out, raise detach.max_inflight, or set null to opt out.",
                    cap
                ),
            }),
        }
    }
}

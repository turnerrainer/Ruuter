//! Source supervision: panic-safe restart with exponential backoff,
//! per-source health state, optional admin route.
//!
//! Every long-lived source task (WebSocket today; future MQTT, Kafka,
//! file-watcher) runs inside a watchdog that:
//!
//! 1. Spawns the source task in `tokio::spawn`.
//! 2. Waits on the `JoinHandle`. On panic OR clean-return-by-error,
//!    records the failure, sleeps with exponential backoff (+ jitter),
//!    and re-spawns. A genuinely clean `Ok(())` return marks the
//!    source `Dead` (intentional shutdown, no restart).
//! 3. Updates a shared `SourceStateMap` so an admin handler can
//!    inspect each source's health without holding a lock on the
//!    runtime.
//!
//! The watchdog is generic — it knows only how to call a `SpawnFn`,
//! observe its `JoinHandle`, and restart. The fact that the underlying
//! source is a WS / Kafka / cron consumer is opaque.

use crate::config::AppConfig;
use crate::sources::{config::SourceConfig, ws};
use crate::triggers::TriggerDispatcher;
use crate::ws::WsRegistry;
use axum::{extract::State, response::IntoResponse, routing::get, Json, Router};
use dashmap::DashMap;
use rand::Rng;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize)]
pub struct SourceId {
    pub project: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceStatus {
    Starting,
    Running,
    Restarting {
        restart_count: u32,
        last_error: String,
        next_attempt_in_ms: u64,
    },
    /// Intentional shutdown — no further restart attempts.
    Dead { reason: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceState {
    pub id: SourceId,
    pub status: SourceStatus,
    pub last_status_change_unix_ms: u64,
    pub restart_count: u32,
}

pub type SourceStateMap = Arc<DashMap<SourceId, SourceState>>;

#[derive(Clone, Debug, Serialize)]
pub struct SupervisorReport {
    pub sources: Vec<SourceState>,
    pub total: usize,
    pub running: usize,
    pub restarting: usize,
    pub dead: usize,
}

#[derive(Clone)]
pub struct SourceSupervisor {
    state: SourceStateMap,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
}

impl SourceSupervisor {
    pub fn new() -> Self {
        Self {
            state: Arc::new(DashMap::new()),
            initial_backoff_ms: 500,
            max_backoff_ms: 60_000,
        }
    }

    /// Spawn one supervised source. Returns the watchdog JoinHandle.
    /// The watchdog itself is what tokio holds; the inner task is
    /// re-spawned on every failure.
    pub fn supervise<F, Fut>(
        &self,
        id: SourceId,
        build: F,
    ) -> JoinHandle<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let state = self.state.clone();
        let initial_backoff = self.initial_backoff_ms;
        let max_backoff = self.max_backoff_ms;

        state.insert(id.clone(), SourceState {
            id: id.clone(),
            status: SourceStatus::Starting,
            last_status_change_unix_ms: now_ms(),
            restart_count: 0,
        });

        tokio::spawn(async move {
            let mut backoff = initial_backoff;
            let mut restart_count: u32 = 0;

            loop {
                set_status(&state, &id, SourceStatus::Running, restart_count);
                let inner = tokio::spawn(build());
                let result = inner.await;

                match result {
                    Ok(()) => {
                        // The source future returned cleanly. For long-
                        // lived sources (WS, MQTT) this means an
                        // intentional shutdown, not a transient error
                        // — don't restart in a tight loop.
                        info!(
                            project = %id.project, name = %id.name, kind = %id.kind,
                            "source future returned Ok; marking dead"
                        );
                        set_status(
                            &state,
                            &id,
                            SourceStatus::Dead { reason: "clean exit".into() },
                            restart_count,
                        );
                        return;
                    }
                    Err(join_err) => {
                        let reason = if join_err.is_panic() {
                            format!("panic: {}", panic_payload(&join_err))
                        } else if join_err.is_cancelled() {
                            "task cancelled".into()
                        } else {
                            "task aborted".into()
                        };
                        if join_err.is_cancelled() {
                            // Cancellation is also an intentional exit
                            // (shutdown / test teardown).
                            set_status(
                                &state,
                                &id,
                                SourceStatus::Dead { reason },
                                restart_count,
                            );
                            return;
                        }
                        restart_count += 1;
                        warn!(
                            project = %id.project, name = %id.name, kind = %id.kind,
                            restart_count, backoff_ms = backoff, %reason,
                            "source crashed; restarting after backoff"
                        );
                        set_status(
                            &state,
                            &id,
                            SourceStatus::Restarting {
                                restart_count,
                                last_error: reason,
                                next_attempt_in_ms: backoff,
                            },
                            restart_count,
                        );
                        sleep_with_jitter(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
            }
        })
    }

    pub fn report(&self) -> SupervisorReport {
        let sources: Vec<SourceState> = self.state
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        let total = sources.len();
        let running = sources.iter().filter(|s| matches!(s.status, SourceStatus::Running)).count();
        let restarting = sources.iter().filter(|s| matches!(s.status, SourceStatus::Restarting { .. })).count();
        let dead = sources.iter().filter(|s| matches!(s.status, SourceStatus::Dead { .. })).count();
        SupervisorReport { sources, total, running, restarting, dead }
    }

    /// Mount the `/_/sources` admin endpoint onto an axum Router.
    /// Caller decides whether to mount it (config-gated in `main.rs`).
    pub fn admin_router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/_/sources", get(handle_sources))
            .with_state(self)
    }
}

impl Default for SourceSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

async fn handle_sources(State(sup): State<Arc<SourceSupervisor>>) -> impl IntoResponse {
    Json(sup.report())
}

fn set_status(state: &SourceStateMap, id: &SourceId, status: SourceStatus, restart_count: u32) {
    let mut entry = state.entry(id.clone()).or_insert_with(|| SourceState {
        id: id.clone(),
        status: SourceStatus::Starting,
        last_status_change_unix_ms: now_ms(),
        restart_count: 0,
    });
    entry.status = status;
    entry.last_status_change_unix_ms = now_ms();
    entry.restart_count = restart_count;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn sleep_with_jitter(ms: u64) {
    let factor: f64 = rand::thread_rng().gen_range(0.5..1.5);
    let jittered = ((ms as f64) * factor) as u64;
    tokio::time::sleep(Duration::from_millis(jittered)).await;
}

fn panic_payload(err: &tokio::task::JoinError) -> String {
    // JoinError doesn't give a stable string for the panic payload, but
    // we can pull a Debug rendering — this only runs on the unhappy
    // path so cost doesn't matter.
    format!("{:?}", err)
}

/// Convenience: spawn every configured source under supervision.
/// Replaces `sources::spawn_all` (which spawned tasks directly).
pub fn supervise_all(
    configs: Vec<(String, String, SourceConfig)>,
    constants: HashMap<String, String>,
    dispatcher: Arc<TriggerDispatcher>,
    ws_registry: WsRegistry,
    sup: &SourceSupervisor,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::with_capacity(configs.len());

    for (project, name, cfg) in configs {
        match cfg {
            SourceConfig::WebSocket(ws_cfg) => {
                let resolved = match crate::sources::config::resolve_constants(&ws_cfg, &constants) {
                    Ok(r) => r,
                    Err(e) => {
                        error!(%project, %name, "source config constant resolution failed: {}", e);
                        continue;
                    }
                };
                let id = SourceId {
                    project: project.clone(),
                    name: name.clone(),
                    kind: "websocket".into(),
                };
                let dispatcher = dispatcher.clone();
                let registry = ws_registry.clone();
                let resolved = Arc::new(resolved);
                let p = project.clone();
                let n = name.clone();
                let handle = sup.supervise(id, move || {
                    let dispatcher = dispatcher.clone();
                    let registry = registry.clone();
                    let resolved = resolved.clone();
                    let p = p.clone();
                    let n = n.clone();
                    async move {
                        ws::run(p, n, (*resolved).clone(), dispatcher, registry).await;
                    }
                });
                handles.push(handle);
                info!(%project, %name, "ws source under supervision");
            }
        }
    }

    handles
}

/// Read whether the admin route should be mounted from app config.
/// Defaulting to disabled — admin endpoint is exposed only when
/// `RUUTER_ADMIN_ENABLED=true` (env var) is set, mirroring the same
/// posture as the existing `internal_requests.disabled` config.
pub fn admin_enabled(_app: &AppConfig) -> bool {
    std::env::var("RUUTER_ADMIN_ENABLED")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

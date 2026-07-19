//! Task 042 — `single_flight` step.
//!
//! Collapses concurrent duplicate requests keyed on a DSL-computed
//! string into ONE execution + N wait-and-share followers. First
//! caller becomes the leader, executes the `do:` body, then
//! broadcasts the outcome via a per-key channel. Followers subscribe
//! before the leader publishes and receive the same value (or a
//! propagated error string).
//!
//! Same-instance only. Two Ruuter replicas each maintain their own
//! registry — a duplicate landing on the other replica is not
//! coalesced. Cross-instance is task 029's shared-store domain.
//!
//! ## Concurrency model
//!
//! ```text
//!   T1 arrives ─▶ DashMap.entry(key).or_insert_with(new_entry)
//!                 is_leader_slot.swap(false)  → true
//!                 → LEADER role, runs body, broadcasts, removes entry
//!
//!   T2 arrives ─▶ DashMap.entry(key).or_insert_with(...)  key present
//!                 is_leader_slot.swap(false)  → false
//!                 → FOLLOWER role, subscribes to broadcast, waits
//!
//!   T3 arrives AFTER T1 removed the entry
//!                 → new insert, is_leader=true → fresh leader
//! ```
//!
//! Race analysis:
//!
//! - T2 subscribing after T1's broadcast but before T1's remove →
//!   T2 will see `RecvError::Closed` (leader dropped sender) and
//!   fall through to a fresh-lead retry inside the same call.
//! - T3 arriving after T1's remove → clean fresh leader.
//! - Leader panics inside `do:` → drop of the Sender closes the
//!   channel; followers see `RecvError::Closed` and treat it as an
//!   error rather than silently deadlocking.

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::engine::StepEngine;
use crate::steps::{SingleFlightStep, StepResult};
use crate::{Result, RuuterError};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Snapshot of what a leader produced. `Ok(Some(v))` = leader
/// finished with a bound result; `Ok(None)` = leader finished with
/// no `result:` variable configured; `Err(msg)` = leader failed
/// (RuuterError isn't Clone, so we downgrade to the display string).
pub type Outcome = std::result::Result<Option<Value>, String>;

/// One slot in the in-flight registry.
pub struct Entry {
    /// Broadcast channel — leader sends exactly one message, then
    /// drops the sender. Followers subscribe before the send; late
    /// subscribers see `RecvError::Closed`.
    tx: broadcast::Sender<Arc<Outcome>>,
    /// `true` until claimed by the first arriving caller. `swap`
    /// atomically transfers leader role to exactly one caller.
    is_leader_slot: AtomicBool,
}

impl Entry {
    fn new() -> Arc<Self> {
        // Capacity 1: leader sends one message, then drops. Higher
        // capacity would only matter if we broadcast multiple
        // outcomes per key, which we don't.
        let (tx, _) = broadcast::channel::<Arc<Outcome>>(1);
        Arc::new(Self {
            tx,
            is_leader_slot: AtomicBool::new(true),
        })
    }
}

/// Process-global (per-StepEngine) registry of in-flight leaders.
/// `Clone`-able because it's just `Arc` inside.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<DashMap<String, Arc<Entry>>>,
    max_entries: usize,
}

impl Registry {
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// Bound the registry so a pathological DSL that generates
    /// unbounded distinct keys (e.g. per-request UUID) can't OOM
    /// the process. On overflow, an arbitrary in-flight slot is
    /// evicted (its followers see Closed and fall through to fresh
    /// leader). Preferred behaviour would be LRU eviction; that
    /// wants an extra data structure and isn't blocking v1.
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            max_entries,
        }
    }

    /// Claim a slot for `key`. Returns `(entry, is_leader)`.
    /// Leaders MUST publish on completion; if they panic or drop
    /// the entry without publishing, followers see
    /// `RecvError::Closed` and fail (which is the correct
    /// propagation of "the leader crashed on the shared input").
    ///
    /// `pub` so integration tests can drive the registry directly
    /// (verify cap enforcement, entry lifetimes). Not part of the
    /// DSL author API — production callers only ever go through
    /// `SingleFlightStepExecutor::execute`.
    pub fn claim(&self, key: &str) -> (Arc<Entry>, bool) {
        // Bounded-size check BEFORE insert. Non-atomic wrt other
        // inserts, so the cap is soft — under contention the map
        // may briefly exceed max_entries by the number of concurrent
        // inserters. That's fine for a memory guard.
        //
        // Eviction uses `retain` (write-lock across all shards) to
        // avoid the DashMap footgun of holding an `iter()` reference
        // while calling `remove()` on the same shard. Under overflow
        // we keep the first `max_entries` we happen to encounter and
        // drop the rest — not LRU, but bounded and deadlock-free.
        if !self.inner.contains_key(key) && self.inner.len() >= self.max_entries {
            let target = self.max_entries;
            let mut kept = 0usize;
            self.inner.retain(|_, _| {
                if kept < target {
                    kept += 1;
                    true
                } else {
                    false
                }
            });
        }

        let entry = self
            .inner
            .entry(key.to_string())
            .or_insert_with(Entry::new)
            .clone();
        let is_leader = entry.is_leader_slot.swap(false, Ordering::AcqRel);
        (entry, is_leader)
    }

    fn remove(&self, key: &str) {
        self.inner.remove(key);
    }

    /// Diagnostic — number of in-flight slots. Used by tests to
    /// assert cleanup happens.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SingleFlightStepExecutor {
    step: SingleFlightStep,
    engine: StepEngine,
    registry: Registry,
    script_engine: ScriptEngine,
}

impl SingleFlightStepExecutor {
    pub fn new(step: SingleFlightStep, engine: StepEngine, registry: Registry) -> Self {
        Self {
            step,
            engine,
            registry,
            script_engine: ScriptEngine::new(),
        }
    }

    /// Not a StepExecutor impl — the trait bounds `execute` as an
    /// `impl Future + Send`, which forces a fixed opaque return
    /// type. This step recurses through StepEngine, which needs
    /// the same Box<dyn Future> escape iterate uses. Callers go
    /// through this async fn directly.
    pub async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let body = &self.step.single_flight;

        // Evaluate the key expression against the caller's context.
        // Keys are DSL-computed (typically `${incoming.body.id}` +
        // constants), so different requests may or may not collide.
        let key_value = self.script_engine.evaluate(&Value::String(body.key.clone()), context)?;
        let key = match key_value {
            Value::String(s) => s,
            other => other.to_string(),
        };

        let (entry, is_leader) = self.registry.claim(&key);

        if is_leader {
            self.run_as_leader(&entry, &key, context).await
        } else {
            self.run_as_follower(&entry, &key, context).await
        }
    }

    async fn run_as_leader(
        &self,
        entry: &Arc<Entry>,
        key: &str,
        context: &ExecutionContext,
    ) -> Result<StepResult> {
        let body = &self.step.single_flight;

        // Run body steps sequentially. Same shape as iterate.do —
        // sub-step `next:` is ignored; a Return step bubbles up.
        let mut early_return: Option<StepResult> = None;
        let mut leader_err: Option<RuuterError> = None;

        for sub in &body.body {
            match self.engine.execute_single_step(sub, context).await {
                Ok(r) => {
                    if r.should_return {
                        early_return = Some(r);
                        break;
                    }
                }
                Err(e) => {
                    leader_err = Some(e);
                    break;
                }
            }
        }

        // Extract the result variable (if configured) from the
        // leader's post-body context.
        let outcome: Outcome = if let Some(err) = &leader_err {
            Err(err.to_string())
        } else if let Some(name) = &body.result {
            Ok(Some(context.get_variable(name).unwrap_or(Value::Null)))
        } else {
            Ok(None)
        };

        // Publish to followers BEFORE removing the entry, so any
        // subscriber that made it through the door sees the value.
        // Ignoring send errors: `Err` here means no followers were
        // subscribed at the moment of send — fine, single-flight
        // just happened to have no coalescing partners this time.
        let _ = entry.tx.send(Arc::new(outcome));
        self.registry.remove(key);

        if let Some(err) = leader_err {
            return Err(err);
        }
        if let Some(early) = early_return {
            return Ok(early);
        }

        // If a result var was configured, the leader's context
        // already has it bound (we snapshotted from there). Nothing
        // more to do beyond advancing to `next:`.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            ..StepResult::new()
        })
    }

    async fn run_as_follower(
        &self,
        entry: &Arc<Entry>,
        key: &str,
        context: &ExecutionContext,
    ) -> Result<StepResult> {
        let body = &self.step.single_flight;
        let mut rx = entry.tx.subscribe();

        let recv = tokio::time::timeout(Duration::from_millis(body.ttl_ms), rx.recv()).await;

        match recv {
            Ok(Ok(shared)) => {
                // Bind the leader's result into this follower's
                // context under the same variable name (if configured).
                match &*shared {
                    Ok(maybe_value) => {
                        if let (Some(name), Some(v)) = (&body.result, maybe_value.clone()) {
                            context.set_variable(name.clone(), v);
                        }
                        Ok(StepResult {
                            next_step: self.step.next.clone(),
                            ..StepResult::new()
                        })
                    }
                    Err(msg) => Err(RuuterError::DslExecution {
                        step: "single_flight".to_string(),
                        message: format!("leader failed: {}", msg),
                    }),
                }
            }
            Ok(Err(_closed)) => {
                // Sender dropped without publishing — leader panicked
                // or broadcast raced with subscribe past the send.
                // Either way, don't silently succeed.
                Err(RuuterError::DslExecution {
                    step: "single_flight".to_string(),
                    message: format!(
                        "leader dropped without publishing (key={:?})",
                        key
                    ),
                })
            }
            Err(_timeout) => {
                // Follower budget exhausted; evict the slot so the
                // next arrival can lead a fresh window rather than
                // piling more followers onto the same hung leader.
                self.registry.remove(key);
                Err(RuuterError::Timeout(format!(
                    "single_flight follower timed out after {}ms (key={:?})",
                    body.ttl_ms, key
                )))
            }
        }
    }
}

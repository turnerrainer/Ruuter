//! Event-trigger dispatcher.
//!
//! Where the HTTP router maps `(method, path)` → DSL, the dispatcher
//! maps `(project, channel, key)` → DSL. The same step engine runs
//! both, so DSL semantics are identical regardless of trigger.
//!
//! Layout on disk:
//!
//! ```text
//! DSL/<project>/triggers/<channel>/<key>.yml
//! DSL/<project>/triggers/<channel>/_default.yml   (matched when <key> has no exact file)
//! ```
//!
//! `channel` and `key` are arbitrary strings. Ruuter has no notion
//! of what a channel represents — `bars`, `device_telemetry`,
//! `chat_messages`, anything goes. Service-specific routing lives in
//! the source config (see #005).

use crate::context::ExecutionContext;
use crate::dsl::loader::TriggerDsls;
use crate::state::StateStore;
use crate::steps::engine::StepEngine;
use crate::Result;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, warn};

const DEFAULT_KEY: &str = "_default";

pub struct TriggerDispatcher {
    triggers: TriggerDsls,
    state: StateStore,
    engine: StepEngine,
}

impl TriggerDispatcher {
    pub fn new(triggers: TriggerDsls, state: StateStore, engine: StepEngine) -> Self {
        Self {
            triggers,
            state,
            engine,
        }
    }

    /// Dispatch an inbound event. `payload` becomes the DSL's request body.
    /// Returns `Ok(true)` if a DSL ran, `Ok(false)` if no matching DSL existed
    /// (no `<key>.yml` and no `_default.yml`). Errors propagate the DSL error.
    pub async fn dispatch(
        &self,
        project: &str,
        channel: &str,
        key: &str,
        payload: Value,
    ) -> Result<bool> {
        let map_key = (project.to_string(), channel.to_string());
        let per_key = match self.triggers.get(&map_key) {
            Some(m) => m,
            None => {
                debug!(project, channel, key, "no triggers registered for channel");
                return Ok(false);
            }
        };

        let dsl = match per_key.get(key).or_else(|| per_key.get(DEFAULT_KEY)) {
            Some(d) => d,
            None => {
                debug!(project, channel, key, "no matching trigger DSL");
                return Ok(false);
            }
        };

        // Build a body HashMap from `payload`. Whatever shape the source
        // sends becomes available as `incoming.body.*` in the DSL.
        let body = match payload {
            Value::Object(map) => map.into_iter().collect::<HashMap<_, _>>(),
            other => {
                // Non-object payloads are wrapped so DSLs can still reach them.
                let mut wrapper = HashMap::new();
                wrapper.insert("value".to_string(), other);
                wrapper
            }
        };

        let context = ExecutionContext::with_state(
            body,
            HashMap::new(),
            HashMap::new(),
            channel.to_string(),
            project.to_string(),
            self.state.clone(),
        )
        .with_expr_registry(self.engine.expr_registry().clone());

        match self.engine.run(dsl, &context).await {
            Ok(result) => {
                if let Some(v) = &result.value {
                    debug!(project, channel, key, ?v, "trigger DSL returned");
                }
                if result.status >= 400 {
                    warn!(
                        project,
                        channel,
                        key,
                        status = result.status,
                        "trigger DSL reported error status"
                    );
                }
                Ok(true)
            }
            Err(e) => {
                // Don't bubble errors up past the dispatcher — the source
                // task should stay alive even if one event's DSL fails.
                warn!(project, channel, key, error = %e, "trigger DSL failed");
                Err(e)
            }
        }
    }
}

impl TriggerDispatcher {
    /// Iterate the (project, channel) pairs for which trigger DSLs are loaded.
    /// Sources use this on startup to know which channels they should subscribe to.
    pub fn channels(&self) -> impl Iterator<Item = (&str, &str)> {
        self.triggers.keys().map(|(p, c)| (p.as_str(), c.as_str()))
    }
}

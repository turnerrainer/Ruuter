//! `ws_tag` step — stamp string tags on the WS connection this DSL
//! run was triggered by.
//!
//! Only meaningful inside a WS server DSL, where
//! `context.connection_id()` is set. The canonical use is to record
//! the authenticated identity resolved on connect:
//!
//! ```yaml
//! stamp_identity:
//!   ws_tag:
//!     set:
//!       user:  "${u.personal_code}"
//!       roles: "${',' + u.roles.join(',') + ','}"   # delimited for token match
//!   next: ack
//! ```
//!
//! A later `ws_send: { broadcast_where: { tag: "roles", contains:
//! ",admin," }, … }` then fans out to exactly those connections —
//! no external "which socket is whom" directory required.

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StepExecutor, StepLogExtras, StepResult, WsTagStep};
use crate::ws::WsRegistry;
use crate::{Result, RuuterError};
use serde_json::Value;

pub struct WsTagStepExecutor {
    step: WsTagStep,
    registry: WsRegistry,
    script_engine: ScriptEngine,
}

impl WsTagStepExecutor {
    pub fn new(step: WsTagStep, registry: WsRegistry) -> Self {
        Self {
            step,
            registry,
            script_engine: ScriptEngine::new(),
        }
    }
}

/// Coerce an evaluated JSON value to the string form stored as a tag.
/// Strings pass through unquoted; null becomes empty; everything else
/// uses its compact JSON rendering.
pub(crate) fn coerce_tag_value(v: Value) -> String {
    match v {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl StepExecutor for WsTagStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let id = context.connection_id().ok_or_else(|| {
            RuuterError::InvalidStep(
                "ws_tag: context has no connection_id — ws_tag is only valid inside a WS server DSL"
                    .into(),
            )
        })?;

        let mut entries: Vec<(String, String)> = Vec::with_capacity(self.step.ws_tag.set.len());
        for (key, value_expr) in &self.step.ws_tag.set {
            let evaluated = self.script_engine.evaluate(value_expr, context)?;
            entries.push((key.clone(), coerce_tag_value(evaluated)));
        }
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        let keys_log = keys.join(",");

        self.registry.set_tags(id, entries)?;

        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new()
                .push("connection_id", id.to_string())
                .push("keys", keys_log),
            ..StepResult::new()
        })
    }
}

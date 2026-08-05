//! `ws_send` step — push a JSON frame to one or more WS connections.
//!
//! Three addressing modes (priority order):
//!   1. `broadcast_prefix: "client:"` — fan-out to every registered
//!      connection whose id starts with the prefix.
//!   2. `to: "<expr>"` — single id or array of ids resolved from the
//!      script engine. Strings are sent verbatim, arrays fan-out.
//!   3. Neither — use `context.connection_id()` (the originating WS
//!      client / source). Errors if the context has no connection id.

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StepExecutor, StepResult, WsSendStep};
use crate::ws::WsRegistry;
use crate::{Result, RuuterError};
use serde_json::Value;

pub struct WsSendStepExecutor {
    step: WsSendStep,
    registry: WsRegistry,
    script_engine: ScriptEngine,
}

impl WsSendStepExecutor {
    pub fn new(step: WsSendStep, registry: WsRegistry) -> Self {
        Self {
            step,
            registry,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for WsSendStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let payload = self
            .script_engine
            .evaluate(&self.step.ws_send.payload, context)?;

        if let Some(prefix) = &self.step.ws_send.broadcast_prefix {
            let delivered = self
                .registry
                .broadcast(|id| id.starts_with(prefix), payload);
            tracing::debug!(prefix, delivered, "ws_send broadcast");
        } else {
            let target_value = match &self.step.ws_send.to {
                Some(expr) => Some(self.script_engine.evaluate(expr, context)?),
                None => context
                    .connection_id()
                    .map(|id| Value::String(id.to_string())),
            };

            let target = target_value.ok_or_else(|| {
                RuuterError::InvalidStep(
                    "ws_send: no `to`, no `broadcast_prefix`, and context has no connection_id"
                        .into(),
                )
            })?;

            match target {
                Value::String(id) => {
                    self.registry.send(&id, payload)?;
                }
                Value::Array(ids) => {
                    for id in ids {
                        let id_str = match id {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        if let Err(e) = self.registry.send(&id_str, payload.clone()) {
                            tracing::warn!(target = %id_str, error = %e, "ws_send: skipping");
                        }
                    }
                }
                other => {
                    return Err(RuuterError::InvalidStep(format!(
                        "ws_send: `to` must resolve to a string or array of strings, got: {}",
                        other
                    )));
                }
            }
        }

        Ok(StepResult {
            next_step: self.step.next.clone(),
            ..StepResult::new()
        })
    }
}

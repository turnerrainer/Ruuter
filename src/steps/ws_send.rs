//! `ws_send` step — push a JSON frame to one or more WS connections.
//!
//! Four addressing modes (priority order):
//!   1. `broadcast_where: { tag: …, equals|contains: … }` — fan-out to
//!      every connection whose tag (set earlier via `ws_tag`) matches.
//!   2. `broadcast_prefix: "client:"` — fan-out to every registered
//!      connection whose id starts with the prefix.
//!   3. `to: "<expr>"` — single id or array of ids resolved from the
//!      script engine. Strings are sent verbatim, arrays fan-out.
//!   4. None of the above — use `context.connection_id()` (the
//!      originating WS client / source). Errors if the context has no
//!      connection id.

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::ws_tag::coerce_tag_value;
use crate::steps::{BroadcastWhere, StepExecutor, StepLogExtras, StepResult, WsSendStep};
use crate::ws::WsRegistry;
use crate::{Result, RuuterError};
use serde_json::Value;
use std::collections::HashMap;

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

        let extras = if let Some(bw) = &self.step.ws_send.broadcast_where {
            let matcher = self.resolve_broadcast_where(bw, context)?;
            let delivered = self.registry.broadcast_where(
                |_id, tags: &HashMap<String, String>| matcher.matches(tags),
                payload,
            );
            tracing::debug!(tag = %matcher.tag, delivered, "ws_send broadcast_where");
            StepLogExtras::new()
                .push("mode", "broadcast_where")
                .push("tag", matcher.tag.clone())
                .push("delivered", delivered as u64)
        } else if let Some(prefix) = &self.step.ws_send.broadcast_prefix {
            let delivered = self
                .registry
                .broadcast(|id| id.starts_with(prefix), payload);
            tracing::debug!(prefix, delivered, "ws_send broadcast");
            StepLogExtras::new()
                .push("mode", "broadcast")
                .push("prefix", prefix.clone())
                .push("delivered", delivered as u64)
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
                    StepLogExtras::new()
                        .push("mode", "unicast")
                        .push("delivered", 1u64)
                }
                Value::Array(ids) => {
                    let mut delivered: u64 = 0;
                    let attempted = ids.len() as u64;
                    for id in ids {
                        let id_str = match id {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        match self.registry.send(&id_str, payload.clone()) {
                            Ok(()) => delivered += 1,
                            Err(e) => {
                                tracing::warn!(target = %id_str, error = %e, "ws_send: skipping");
                            }
                        }
                    }
                    StepLogExtras::new()
                        .push("mode", "fan-out")
                        .push("attempted", attempted)
                        .push("delivered", delivered)
                }
                other => {
                    return Err(RuuterError::InvalidStep(format!(
                        "ws_send: `to` must resolve to a string or array of strings, got: {}",
                        other
                    )));
                }
            }
        };

        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

/// A `broadcast_where` predicate with every `${…}` operand already
/// resolved against the current context.
struct ResolvedWhere {
    tag: String,
    op: WhereOp,
}

enum WhereOp {
    Equals(String),
    Contains(String),
}

impl ResolvedWhere {
    fn matches(&self, tags: &HashMap<String, String>) -> bool {
        match tags.get(&self.tag) {
            None => false,
            Some(v) => match &self.op {
                WhereOp::Equals(want) => v == want,
                WhereOp::Contains(needle) => v.contains(needle.as_str()),
            },
        }
    }
}

impl WsSendStepExecutor {
    fn resolve_broadcast_where(
        &self,
        bw: &BroadcastWhere,
        context: &ExecutionContext,
    ) -> Result<ResolvedWhere> {
        let tag = coerce_tag_value(self.script_engine.evaluate(&bw.tag, context)?);
        if tag.is_empty() {
            return Err(RuuterError::InvalidStep(
                "ws_send: broadcast_where.tag resolved to an empty string".into(),
            ));
        }
        let op = match (&bw.equals, &bw.contains) {
            (Some(_), Some(_)) => {
                return Err(RuuterError::InvalidStep(
                    "ws_send: broadcast_where takes exactly one of `equals` / `contains`".into(),
                ))
            }
            (Some(e), None) => {
                WhereOp::Equals(coerce_tag_value(self.script_engine.evaluate(e, context)?))
            }
            (None, Some(c)) => {
                WhereOp::Contains(coerce_tag_value(self.script_engine.evaluate(c, context)?))
            }
            (None, None) => {
                return Err(RuuterError::InvalidStep(
                    "ws_send: broadcast_where needs one of `equals` / `contains`".into(),
                ))
            }
        };
        Ok(ResolvedWhere { tag, op })
    }
}

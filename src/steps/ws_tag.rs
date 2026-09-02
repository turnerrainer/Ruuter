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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{BaseStepFields, WsTagArgs};
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};

    fn ctx_without_connection() -> ExecutionContext {
        ExecutionContext::new(HashMap::new(), HashMap::new(), HashMap::new(), String::new())
    }

    fn ctx_with_connection(id: &str) -> ExecutionContext {
        ExecutionContext::new(HashMap::new(), HashMap::new(), HashMap::new(), String::new())
            .with_connection_id(id.to_string())
    }

    fn step(entries: &[(&str, Value)]) -> WsTagStep {
        let mut set = BTreeMap::new();
        for (k, v) in entries {
            set.insert((*k).to_string(), v.clone());
        }
        WsTagStep {
            ws_tag: WsTagArgs { set },
            next: None,
            base: BaseStepFields::default(),
        }
    }

    #[test]
    fn coerce_tag_value_covers_json_shapes() {
        assert_eq!(coerce_tag_value(json!("abc")), "abc");
        // String pass-through — no surrounding JSON quotes added.
        assert_eq!(coerce_tag_value(json!("\"quoted\"")), "\"quoted\"");
        assert_eq!(coerce_tag_value(Value::Null), "");
        assert_eq!(coerce_tag_value(json!(42)), "42");
        assert_eq!(coerce_tag_value(json!(true)), "true");
        assert_eq!(coerce_tag_value(json!(["a", "b"])), "[\"a\",\"b\"]");
        assert_eq!(coerce_tag_value(json!({"k":"v"})), "{\"k\":\"v\"}");
    }

    #[tokio::test]
    async fn errors_when_context_has_no_connection_id() {
        let exec = WsTagStepExecutor::new(step(&[("k", json!("v"))]), WsRegistry::new());
        let err = exec
            .execute(&ctx_without_connection())
            .await
            .expect_err("expected InvalidStep outside a WS DSL");
        assert!(err.to_string().contains("connection_id"), "got: {err}");
    }

    #[tokio::test]
    async fn errors_when_connection_is_not_registered() {
        // Context claims a connection id, but the registry has no
        // matching entry — must fail loudly, not silently drop.
        let exec = WsTagStepExecutor::new(step(&[("k", json!("v"))]), WsRegistry::new());
        let err = exec
            .execute(&ctx_with_connection("client:ghost"))
            .await
            .expect_err("expected InvalidStep for unknown connection");
        assert!(err.to_string().contains("client:ghost"), "got: {err}");
    }

    #[tokio::test]
    async fn writes_evaluated_values_to_registry() {
        let reg = WsRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        reg.register("client:x".into(), tx);

        let exec = WsTagStepExecutor::new(
            step(&[
                ("user", json!("alice")),
                ("roles", json!(",admin,ops,")),
                ("nullable", Value::Null),
            ]),
            reg.clone(),
        );
        exec.execute(&ctx_with_connection("client:x"))
            .await
            .expect("tag write");

        let tags = reg.tags_of("client:x").expect("registered");
        assert_eq!(tags.get("user").map(String::as_str), Some("alice"));
        assert_eq!(tags.get("roles").map(String::as_str), Some(",admin,ops,"));
        assert_eq!(tags.get("nullable").map(String::as_str), Some(""));
    }

    #[tokio::test]
    async fn empty_set_is_a_valid_noop_on_a_registered_connection() {
        let reg = WsRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        reg.register("client:x".into(), tx);

        let exec = WsTagStepExecutor::new(step(&[]), reg.clone());
        exec.execute(&ctx_with_connection("client:x"))
            .await
            .expect("empty set should succeed");
        assert!(reg.tags_of("client:x").unwrap().is_empty());
    }
}

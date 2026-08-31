use crate::config::LoggingConfig;
use crate::context::ExecutionContext;
use crate::logging::preview_body_for_log;
use crate::scripting::ScriptEngine;
use crate::steps::{StateOp, StateStep, StepExecutor, StepLogExtras, StepResult};
use crate::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct StateStepExecutor {
    step: StateStep,
    script_engine: ScriptEngine,
    /// Threaded through so `state.value` on set ops honours the
    /// framework-configured `redact_body_fields` — a project that
    /// adds project-specific PII / secret names must see them
    /// redacted here too. Falls back to defaults when unset.
    logging: Arc<LoggingConfig>,
}

impl StateStepExecutor {
    pub fn new(step: StateStep) -> Self {
        Self::with_logging(step, Arc::new(LoggingConfig::default()))
    }

    pub fn with_logging(step: StateStep, logging: Arc<LoggingConfig>) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
            logging,
        }
    }
}

impl StepExecutor for StateStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let project = context.project();
        let store = context.state();

        // Issue #37 — extras captured so the engine's "Executed" INFO
        // line records the op + key. `hit` reports whether a Get
        // resolved to a present key (`true`) or fell back to null
        // (`false`) — the single most-asked question when a DSL
        // reading state produces "unexpected null". On sets we also
        // surface a redacted preview of the value written, so the
        // log line answers "what did I actually store?" without a
        // second trip through the state store.
        let (op, extras_key, hit, set_preview) = match &self.step.state {
            StateOp::Get { key, into } => {
                let key = self.evaluate_string(key, context)?;
                let value = store.get(project, &key);
                let hit = value.is_some();
                context.set_variable(into.clone(), value.unwrap_or(Value::Null));
                ("get", key, Some(hit), None)
            }
            StateOp::Set { key, value } => {
                let key = self.evaluate_string(key, context)?;
                let evaluated = self.script_engine.evaluate(value, context)?;
                let preview = preview_body_for_log(Some(&evaluated), &self.logging);
                store.set(project, &key, evaluated);
                ("set", key, None, preview)
            }
            StateOp::Delete { key } => {
                let key = self.evaluate_string(key, context)?;
                store.delete(project, &key);
                ("delete", key, None, None)
            }
        };

        let mut extras = StepLogExtras::new().push("op", op).push("key", extras_key);
        if let Some(hit) = hit {
            extras = extras.push("hit", hit);
        }
        if let Some(preview) = set_preview {
            extras = extras.push_preformatted("value", preview);
        }
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

impl StateStepExecutor {
    fn evaluate_string(&self, raw: &str, context: &ExecutionContext) -> Result<String> {
        let value = self
            .script_engine
            .evaluate(&Value::String(raw.to_string()), context)?;
        Ok(match value {
            Value::String(s) => s,
            other => other.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::steps::{DslStep, StateOp};
    use serde_yaml_ng::Value as YamlValue;

    // Mirrors the loader path in src/dsl/parser.rs: raw YAML → YamlValue
    // → from_value into DslStep. The DslStep untagged enum wrapper is
    // what lets serde_yaml_ng accept single-key-map external tags for
    // StateOp; parsing directly into StateStep hits Value::Tagged
    // machinery and rejects plain maps.
    fn parse_state_op(yaml: &str) -> StateOp {
        let value: YamlValue = serde_yaml_ng::from_str(yaml).expect("parse YAML");
        let step: DslStep = serde_yaml_ng::from_value(value).expect("DslStep from_value");
        match step {
            DslStep::State(s) => s.state,
            other => panic!("expected DslStep::State, got {:?}", other),
        }
    }

    /// The canonical `delete:` YAML key deserializes into `StateOp::Delete`.
    #[test]
    fn state_delete_yaml_deserializes() {
        let op = parse_state_op("state:\n  delete:\n    key: k\n");
        assert!(matches!(op, StateOp::Delete { .. }));
    }

    /// The `remove:` alias deserializes into the same `StateOp::Delete`
    /// variant. Guards against a Serde behaviour change silently
    /// dropping the alias.
    #[test]
    fn state_remove_yaml_deserializes_as_delete_alias() {
        let op = parse_state_op("state:\n  remove:\n    key: k\n");
        assert!(
            matches!(op, StateOp::Delete { .. }),
            "`remove:` must be an alias for `delete:`; got a different variant"
        );
    }
}

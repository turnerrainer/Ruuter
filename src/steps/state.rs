use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StateOp, StateStep, StepExecutor, StepResult};
use crate::Result;
use serde_json::Value;

pub struct StateStepExecutor {
    step: StateStep,
    script_engine: ScriptEngine,
}

impl StateStepExecutor {
    pub fn new(step: StateStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for StateStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let project = context.project();
        let store = context.state();

        match &self.step.state {
            StateOp::Get { key, into } => {
                let key = self.evaluate_string(key, context)?;
                let value = store.get(project, &key).unwrap_or(Value::Null);
                context.set_variable(into.clone(), value);
            }
            StateOp::Set { key, value } => {
                let key = self.evaluate_string(key, context)?;
                let evaluated = self.script_engine.evaluate(value, context)?;
                store.set(project, &key, evaluated);
            }
            StateOp::Delete { key } => {
                let key = self.evaluate_string(key, context)?;
                store.delete(project, &key);
            }
        }

        Ok(StepResult {
            next_step: self.step.next.clone(),
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

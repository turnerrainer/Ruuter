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

        Ok(StepResult::with_next(
            self.step.next.clone().unwrap_or_else(|| "end".to_string()),
        ))
    }
}

impl StateStepExecutor {
    fn evaluate_string(&self, raw: &str, context: &ExecutionContext) -> Result<String> {
        let value = self.script_engine
            .evaluate(&Value::String(raw.to_string()), context)?;
        Ok(match value {
            Value::String(s) => s,
            other => other.to_string(),
        })
    }
}

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{ReturnStep, StepExecutor, StepResult};
use crate::Result;
use std::collections::HashMap;

pub struct ReturnStepExecutor {
    step: ReturnStep,
    script_engine: ScriptEngine,
}

impl ReturnStepExecutor {
    pub fn new(step: ReturnStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for ReturnStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let return_value = self.script_engine.evaluate(&self.step.return_value, context)?;

        // status: literal u16 or script expression that resolves to a number
        let status = if let Some(s) = &self.step.status {
            let evaluated = self.script_engine.evaluate(s, context)?;
            match evaluated {
                serde_json::Value::Number(n) => n.as_u64().map(|u| u as u16),
                _ => None,
            }
        } else {
            None
        };

        let headers = if let Some(h) = &self.step.headers {
            let mut evaluated = HashMap::new();
            for (k, v) in h {
                let val = self.script_engine.evaluate(v, context)?;
                evaluated.insert(k.clone(), match val {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                });
            }
            Some(evaluated)
        } else {
            None
        };

        Ok(StepResult::with_return(
            return_value,
            status,
            headers,
        ))
    }
}

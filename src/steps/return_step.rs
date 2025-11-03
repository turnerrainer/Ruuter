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

#[async_trait::async_trait]
impl StepExecutor for ReturnStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let return_value = self.script_engine.evaluate(&self.step.return_value, context)?;

        let headers = if let Some(h) = &self.step.headers {
            let mut evaluated = HashMap::new();
            for (k, v) in h {
                let val = self.script_engine.evaluate(v, context)?;
                evaluated.insert(k.clone(), val.to_string());
            }
            Some(evaluated)
        } else {
            None
        };

        Ok(StepResult::with_return(
            return_value,
            self.step.status,
            headers,
        ))
    }
}

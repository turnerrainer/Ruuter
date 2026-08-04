use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{LogStep, StepExecutor, StepResult};
use crate::Result;
use tracing::info;

pub struct LogStepExecutor {
    step: LogStep,
    script_engine: ScriptEngine,
}

impl LogStepExecutor {
    pub fn new(step: LogStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for LogStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let message = self
            .script_engine
            .evaluate(&serde_json::Value::String(self.step.log.clone()), context)?;

        info!("LOG: {}", message);

        Ok(StepResult {
            next_step: self.step.next.clone(),
            ..StepResult::new()
        })
    }
}

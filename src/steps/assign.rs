use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{AssignStep, StepExecutor, StepResult};
use crate::Result;

pub struct AssignStepExecutor {
    step: AssignStep,
    script_engine: ScriptEngine,
}

impl AssignStepExecutor {
    pub fn new(step: AssignStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for AssignStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        for (key, value) in &self.step.assign {
            let evaluated = self.script_engine.evaluate(value, context)?;
            context.set_variable(key.clone(), evaluated);
        }

        Ok(StepResult::with_next(
            self.step.next.clone().unwrap_or_else(|| "end".to_string()),
        ))
    }
}

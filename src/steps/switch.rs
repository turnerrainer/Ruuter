use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StepExecutor, StepResult, SwitchStep};
use crate::Result;

pub struct SwitchStepExecutor {
    step: SwitchStep,
    script_engine: ScriptEngine,
}

impl SwitchStepExecutor {
    pub fn new(step: SwitchStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for SwitchStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        for condition in &self.step.switch {
            let result = self.script_engine.evaluate(
                &serde_json::Value::String(condition.condition.clone()),
                context,
            )?;

            if let Some(b) = result.as_bool() {
                if b {
                    return Ok(StepResult::with_next(condition.next.clone()));
                }
            }
        }

        Ok(StepResult::with_next(
            self.step.next.clone().unwrap_or_else(|| "end".to_string()),
        ))
    }
}

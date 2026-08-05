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

        // Finding 03 fix: no condition matched — fall through to
        // `next:` if set, else to source-order next.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            ..StepResult::new()
        })
    }
}

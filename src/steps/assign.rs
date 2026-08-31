use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{AssignStep, StepExecutor, StepLogExtras, StepResult};
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
        let mut assigned: Vec<String> = Vec::with_capacity(self.step.assign.len());
        for (key, value) in &self.step.assign {
            let evaluated = self.script_engine.evaluate(value, context)?;
            context.set_variable(key.clone(), evaluated);
            assigned.push(key.clone());
        }
        // Sort for stable log output — HashMap iteration order isn't
        // guaranteed across runs and readers diffing two logs
        // shouldn't see spurious reordering.
        assigned.sort();

        // Finding 03 fix: return `next` as-is (None → engine falls
        // through to source-order next). Never force `"end"`.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new().push("keys", assigned.join(",")),
            ..StepResult::new()
        })
    }
}

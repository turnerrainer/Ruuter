use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StepExecutor, StepLogExtras, StepResult, SwitchStep};
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
        for (idx, condition) in self.step.switch.iter().enumerate() {
            let result = self.script_engine.evaluate(
                &serde_json::Value::String(condition.condition.clone()),
                context,
            )?;

            if let Some(b) = result.as_bool() {
                if b {
                    // Issue #37 — surface which condition matched so
                    // the engine's "Executed" INFO line explains WHY
                    // the DSL jumped where it did. `condition` is the
                    // 0-indexed slot the DSL's `switch:` list is
                    // keyed by; `expr` is the raw JS expression at
                    // that slot — together they let a reader locate
                    // the branch in the DSL file without opening it.
                    return Ok(StepResult {
                        next_step: Some(condition.next.clone()),
                        log_extras: StepLogExtras::new()
                            .push("condition", idx as u64)
                            .push("expr", condition.condition.clone()),
                        ..StepResult::new()
                    });
                }
            }
        }

        // Finding 03 fix: no condition matched — fall through to
        // `next:` if set, else to source-order next.
        //
        // `condition=undefined` (unquoted, JS-native "no value"
        // sentinel) keeps the field name consistent with the match
        // case: readers filtering on `condition=` catch both branches
        // and can distinguish match (`condition=<n>`) from no-match
        // (`condition=undefined`) with the same predicate.
        // `push_preformatted` bypasses the string-quoting Display
        // would apply — we want `condition=undefined`, not
        // `condition="undefined"`.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new().push_preformatted("condition", "undefined"),
            ..StepResult::new()
        })
    }
}

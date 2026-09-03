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
        // Emitted as `condition=no_match` (unquoted, snake_case
        // sentinel matching the rest of Ruuter's log-attr casing).
        // Field name stays `condition=` so a single grep predicate
        // catches both branches and can distinguish match
        // (`condition=<n>`) from no-match (`condition=no_match`).
        // `push_preformatted` bypasses the string-quoting Display
        // would apply — we want `condition=no_match`, not
        // `condition="no_match"`. Historic note: an earlier iteration
        // (#37) used `condition=undefined` for JS-native symmetry,
        // renamed for readability on the back of #54.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new().push_preformatted("condition", "no_match"),
            ..StepResult::new()
        })
    }
}

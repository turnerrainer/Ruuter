use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{StepExecutor, StepLogExtras, StepResult, SwitchStep};
use crate::Result;
use serde_json::Value;

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

/// Issue #64 — a switch condition matches whenever its evaluated
/// value is JS-truthy, not only when it is literally `true`. Mirrors
/// ECMAScript §7.1.5 ToBoolean:
///
/// - `null` / `undefined` → false
/// - boolean → itself
/// - `0` / `NaN` → false; any other number → true
/// - `""` → false; any other string → true
/// - Any object or array (even empty) → true
///
/// This diverges from Java Ruuter, which uses
/// `Boolean.TRUE.equals(...)` — strict reference equality with the
/// boxed boolean. See DIVERGENCES.md entry D-40. The reason: the
/// expression language is JavaScript, `${a && b}` returns `b` (not
/// `true`) when `a` is truthy, and DSL authors reasonably expect
/// their conditions to fire without wrapping every non-boolean
/// expression in `!!(...)`.
pub(crate) fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

impl StepExecutor for SwitchStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        for (idx, condition) in self.step.switch.iter().enumerate() {
            let result = self
                .script_engine
                .evaluate(&Value::String(condition.condition.clone()), context)?;

            if is_truthy(&result) {
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

#[cfg(test)]
mod truthy_tests {
    use super::is_truthy;
    use serde_json::json;

    #[test]
    fn null_and_undefined_are_falsy() {
        // Value::Null covers both JSON null and JS undefined (undefined
        // crosses the JSON boundary as null).
        assert!(!is_truthy(&json!(null)));
    }

    #[test]
    fn booleans_pass_through() {
        assert!(is_truthy(&json!(true)));
        assert!(!is_truthy(&json!(false)));
    }

    #[test]
    fn zero_is_falsy_other_finite_numbers_truthy() {
        // serde_json refuses to encode NaN / +Inf / -Inf as a Number
        // (JSON spec has no representation), so those cases are
        // unreachable through Value::Number. The `as_f64` guard in
        // is_truthy handles them defensively regardless.
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!(0.0)));
        assert!(!is_truthy(&json!(-0.0)));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!(-1)));
        assert!(is_truthy(&json!(0.1)));
        assert!(is_truthy(&json!(-3.14)));
    }

    #[test]
    fn empty_string_is_falsy_others_truthy() {
        assert!(!is_truthy(&json!("")));
        assert!(is_truthy(&json!("hello")));
        assert!(is_truthy(&json!(" ")));
        assert!(is_truthy(&json!("0"))); // JS: Boolean("0") === true
        assert!(is_truthy(&json!("false"))); // JS: Boolean("false") === true
    }

    #[test]
    fn arrays_and_objects_are_truthy_even_when_empty() {
        // JS parity: [] is truthy, {} is truthy. Only the primitive
        // "empty" values (0 / "" / null / undefined) are falsy.
        assert!(is_truthy(&json!([])));
        assert!(is_truthy(&json!([1, 2])));
        assert!(is_truthy(&json!({})));
        assert!(is_truthy(&json!({"a": 1})));
    }
}

//! JSON matchers used by the test runner.
//!
//! - `deep_equal(expected, actual)` — recursive equality.
//! - `subset_matches(expected, actual)` — every key/index in `expected`
//!   must be present in `actual` with a matching value. Extras in
//!   `actual` are ignored. Wildcards: string `"***"` matches anything;
//!   `"$type:<t>"` matches by JSON type; `"$regex:<pat>"` matches a
//!   string against a regex.
//!
//! The subset form is what `body_matches:` and `verify_state:` use.

use regex::Regex;
use serde_json::Value;

/// Deep-equal on JSON values, with no wildcard support. Used when the
/// test declares `body:` (exact match).
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    a == b
}

/// Subset match. `expected` must be a subset of `actual`:
///
/// - Both objects: every key in `expected` exists in `actual` and its
///   value subset-matches.
/// - Both arrays: same length AND each pair subset-matches. (An array
///   subset by position is a decent default; use "***" per element to
///   opt out of a specific position.)
/// - Scalars: equal, OR `expected` is a wildcard string.
pub fn subset_matches(expected: &Value, actual: &Value) -> bool {
    // Wildcard on the expected side always matches.
    if let Value::String(s) = expected {
        if s == "***" {
            return true;
        }
        if let Some(rest) = s.strip_prefix("$type:") {
            return type_matches(rest, actual);
        }
        if let Some(pat) = s.strip_prefix("$regex:") {
            if let Value::String(text) = actual {
                if let Ok(re) = Regex::new(pat) {
                    return re.is_match(text);
                }
            }
            return false;
        }
    }

    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, ev)| a.get(k).map(|av| subset_matches(ev, av)).unwrap_or(false)),
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a.iter()).all(|(ev, av)| subset_matches(ev, av))
        }
        // Numeric tolerance: `400` and `400.0` should compare equal
        // even though serde_json stores them as different Number
        // variants. Falls back to strict equality on non-number pairs.
        (Value::Number(e), Value::Number(a)) => match (e.as_f64(), a.as_f64()) {
            (Some(ef), Some(af)) => (ef - af).abs() < 1e-9,
            _ => e == a,
        },
        _ => expected == actual,
    }
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "bool" | "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        "any" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subset_ignores_extras() {
        let expected = json!({ "a": 1 });
        let actual = json!({ "a": 1, "b": 2 });
        assert!(subset_matches(&expected, &actual));
    }

    #[test]
    fn subset_rejects_missing() {
        let expected = json!({ "a": 1, "b": 2 });
        let actual = json!({ "a": 1 });
        assert!(!subset_matches(&expected, &actual));
    }

    #[test]
    fn subset_wildcard_matches_any_value() {
        let expected = json!({ "a": "***" });
        let actual = json!({ "a": { "nested": true } });
        assert!(subset_matches(&expected, &actual));
    }

    #[test]
    fn subset_type_matcher() {
        assert!(subset_matches(&json!("$type:number"), &json!(42)));
        assert!(!subset_matches(&json!("$type:number"), &json!("42")));
    }

    #[test]
    fn subset_regex_matcher() {
        assert!(subset_matches(
            &json!("$regex:^txn-\\d+$"),
            &json!("txn-12345")
        ));
    }
}

//! Task 037 — literal fast-path in `ScriptEngine::evaluate()`.
//!
//! Contract: for any input value that recursively contains no `${...}`
//! and is not a whole-string `$= expr =` (per the LINE_PATTERN regex
//! where `$` is end-of-string), `evaluate_tracked()` must:
//!   1. Return the input unchanged (byte-identical).
//!   2. Report `boa_used = false`.
//!
//! For any input that DOES contain an expression, it must:
//!   1. Produce the same result the pre-037 engine did.
//!   2. Report `boa_used = true`.
//!
//! Uses `evaluate_tracked()` (not the global `BOA_CONTEXT_CREATED_COUNT`
//! atomic) so parallel test files that also exercise ScriptEngine don't
//! race with these assertions.

use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::scripting::ScriptEngine;
use serde_json::json;
use std::collections::HashMap;

fn ctx() -> ExecutionContext {
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
}

// ── Fast-path: NO Boa context created ────────────────────────────────

#[test]
fn pure_string_literal_takes_fastpath() {
    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!("plain text"), &ctx())
        .unwrap();
    assert_eq!(out, json!("plain text"));
    assert!(!boa_used, "Boa must NOT be constructed for a plain string");
}

#[test]
fn scalar_literals_take_fastpath() {
    let engine = ScriptEngine::new();
    for s in &[
        json!(42),
        json!(-2.5),
        json!(true),
        json!(false),
        json!(null),
    ] {
        let (out, boa_used) = engine.evaluate_tracked(s, &ctx()).unwrap();
        assert_eq!(&out, s, "scalar {:?} must pass through unchanged", s);
        assert!(!boa_used, "Boa must NOT be constructed for scalar {:?}", s);
    }
}

#[test]
fn deeply_nested_literals_take_fastpath() {
    let deep = json!({
        "a": {"b": {"c": {"d": {"e": "still literal"}}}},
        "arr": [[[[[42]]]]],
        "mix": [{"k": "v"}, {"k": "v2"}, [1, 2, [3, 4]]],
    });
    let (out, boa_used) = ScriptEngine::new().evaluate_tracked(&deep, &ctx()).unwrap();
    assert_eq!(out, deep, "deeply-nested literal tree must pass through");
    assert!(
        !boa_used,
        "Boa must NOT be constructed for deep literal tree"
    );
}

#[test]
fn empty_containers_take_fastpath() {
    let engine = ScriptEngine::new();
    for c in &[
        json!({}),
        json!([]),
        json!(""),
        json!({"a": []}),
        json!([{}]),
    ] {
        let (out, boa_used) = engine.evaluate_tracked(c, &ctx()).unwrap();
        assert_eq!(&out, c);
        assert!(!boa_used, "empty {:?} must skip Boa", c);
    }
}

#[test]
fn unicode_literal_takes_fastpath() {
    let v = json!({"greeting": "Tere, maailm! 你好 🚚"});
    let (out, boa_used) = ScriptEngine::new().evaluate_tracked(&v, &ctx()).unwrap();
    assert_eq!(out, v);
    assert!(!boa_used);
}

#[test]
fn partial_dollar_delimiters_do_not_trigger_boa() {
    // LINE_PATTERN is `\$=(.+)=$` — regex `$` = end-of-string, so
    // the closing delimiter is a single `=`, not `=$`. Anything else
    // is a literal.
    let engine = ScriptEngine::new();
    let cases = [
        "trailing = but no start", // no leading $=
        "$=",                      // 2 chars, no closing
        "$=$",                     // 3 chars, ends with $, not =
        "$=x=$",                   // 5 chars, ends with $, not =
        "just a $ sign",
        "text with { curly } braces but no dollar",
    ];
    for c in &cases {
        let (out, boa_used) = engine.evaluate_tracked(&json!(c), &ctx()).unwrap();
        assert_eq!(out, json!(c), "input {:?} must pass through literally", c);
        assert!(!boa_used, "Boa must NOT be constructed for {:?}", c);
    }
}

// ── Slow path: Boa context IS created ────────────────────────────────

#[test]
fn simple_expression_dispatches_to_boa() {
    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!("${1 + 1}"), &ctx())
        .unwrap();
    assert_eq!(out, json!(2));
    assert!(boa_used, "Boa must be constructed for ${{1+1}}");
}

#[test]
fn line_pattern_dispatches_to_boa() {
    // LINE_PATTERN regex trailing anchor is `$` (end-of-string),
    // so the closing delimiter is a single `=`.
    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!("$=41 + 1="), &ctx())
        .unwrap();
    assert_eq!(out, json!(42));
    assert!(boa_used, "Boa must be constructed for $=...= line pattern");
}

#[test]
fn expression_in_deeply_nested_value_dispatches_to_boa() {
    let v = json!({
        "outer": {
            "static": "literal",
            "nested": {"expr": "${'hello ' + 'world'}"}
        }
    });
    let (out, boa_used) = ScriptEngine::new().evaluate_tracked(&v, &ctx()).unwrap();
    assert_eq!(out["outer"]["static"], "literal");
    assert_eq!(out["outer"]["nested"]["expr"], "hello world");
    assert!(
        boa_used,
        "Boa must be constructed when any leaf has an expression"
    );
}

#[test]
fn unbalanced_open_brace_still_takes_slow_path_safely() {
    // `${` triggers slow path (conservative). Slow path handles the
    // unbalanced case by leaving the string as-is.
    let input = "${ unbalanced";
    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!(input), &ctx())
        .unwrap();
    assert_eq!(out, json!(input), "unbalanced input must pass through");
    assert!(
        boa_used,
        "unbalanced ${{ is conservative — slow path DOES fire, harmlessly"
    );
}

#[test]
fn one_expr_in_a_hundred_literals_creates_one_context() {
    let mut m = serde_json::Map::new();
    for i in 0..99 {
        m.insert(format!("k{}", i), json!(format!("literal-{}", i)));
    }
    m.insert("dynamic".into(), json!("${42}"));

    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!(m), &ctx())
        .unwrap();
    assert_eq!(out["dynamic"], 42);
    assert_eq!(out["k50"], "literal-50");
    assert!(boa_used, "one expression anywhere → Boa constructed");
}

// ── Correctness: results identical to pre-037 behavior ───────────────

#[test]
fn mixed_literal_and_expression_evaluates_both_correctly() {
    let engine = ScriptEngine::new();
    let input = json!({
        "static_string":   "no expression here",
        "static_num":      42,
        "static_bool":     true,
        "static_null":     null,
        "static_arr":      [1, "two", true, null],
        "dynamic_string":  "${'hello ' + 'world'}",
        "dynamic_num":     "${21 * 2}",
        "dynamic_arr":     "${[1, 2, 3].map(x => x * 10)}",
        "nested": {
            "still_static":  ["a", "b", "c"],
            "still_dynamic": "${41 + 1}"
        }
    });
    let out = engine.evaluate(&input, &ctx()).unwrap();

    assert_eq!(out["static_string"], "no expression here");
    assert_eq!(out["static_num"], 42);
    assert_eq!(out["static_bool"], true);
    assert!(out["static_null"].is_null());
    assert_eq!(out["static_arr"], json!([1, "two", true, null]));
    assert_eq!(out["dynamic_string"], "hello world");
    assert_eq!(out["dynamic_num"], 42);
    assert_eq!(out["dynamic_arr"], json!([10, 20, 30]));
    assert_eq!(out["nested"]["still_static"], json!(["a", "b", "c"]));
    assert_eq!(out["nested"]["still_dynamic"], 42);
}

#[test]
fn incoming_body_reference_still_works() {
    let engine = ScriptEngine::new();
    let mut body = HashMap::new();
    body.insert("name".to_string(), json!("Ada"));
    body.insert("count".to_string(), json!(7));
    let c = ExecutionContext::new(body, HashMap::new(), HashMap::new(), "test".into());

    let out = engine
        .evaluate(
            &json!("hello ${incoming.body.name} (x${incoming.body.count})"),
            &c,
        )
        .unwrap();
    assert_eq!(out, "hello Ada (x7)");
}

#[test]
fn empty_string_and_pure_dollar_signs_stay_literal() {
    let engine = ScriptEngine::new();
    for s in &["", "$", "$$$"] {
        let (out, boa_used) = engine.evaluate_tracked(&json!(s), &ctx()).unwrap();
        assert_eq!(out, json!(s));
        assert!(!boa_used, "{:?} must be literal", s);
    }
}

// ── Regression: task 022 (script-segment walker skips JS string literals)
#[test]
fn object_literal_inside_expression_still_dispatches() {
    let (out, boa_used) = ScriptEngine::new()
        .evaluate_tracked(&json!("${({ok: true, count: 1 + 1})}"), &ctx())
        .unwrap();
    assert_eq!(out, json!({"ok": true, "count": 2}));
    assert!(boa_used);
}

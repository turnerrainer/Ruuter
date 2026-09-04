//! Issue #62 — `??` and `?.` fire correctly with top-level
//! undeclared identifiers.
//!
//! Under #57, `${platform?.id}` where `platform` is undeclared
//! evaluated to `undefined` (→ `Value::Null`). That fix wrapped the
//! whole expression in a try/catch that swallowed ReferenceError and
//! returned undefined for the *whole* expression — which broke
//! nullish-coalescing: `${missing_var?.blah ?? '123'}` returned
//! `undefined` instead of `'123'`, because the outermost catch
//! meant `??` never got a chance to see `undefined?.blah` and pick
//! the fallback.
//!
//! The fix (retry-with-declaration) treats undeclared identifiers as
//! `undefined` *within the expression*, so `??` and `?.` evaluate
//! naturally under JS semantics.

use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::scripting::ScriptEngine;
use serde_json::Value;
use std::collections::HashMap;

fn empty_ctx() -> ExecutionContext {
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
}

/// Reporter's exact case: undeclared top-level identifier with `?.`
/// and `??` fallback. Must return the fallback, not null.
#[test]
fn nullish_coalescing_with_optional_chain_on_undeclared_returns_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${missing_var?.blah ?? '123'}".into()),
            &empty_ctx(),
        )
        .expect("undeclared identifier with ?? fallback must not throw");
    assert_eq!(
        out,
        Value::String("123".into()),
        "`${{missing_var?.blah ?? '123'}}` must evaluate to '123', not null"
    );
}

/// Simpler variant: bare `??` fallback on an undeclared identifier.
/// `undefined ?? '123'` should return `'123'`.
#[test]
fn nullish_coalescing_on_bare_undeclared_returns_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${missing_var ?? 'fallback'}".into()),
            &empty_ctx(),
        )
        .expect("undeclared identifier with ?? fallback must not throw");
    assert_eq!(out, Value::String("fallback".into()));
}

/// `?.` on undeclared, but no `??`. Should still return `undefined`
/// (`null` at the JSON boundary) per #57. Regression guard.
#[test]
fn optional_chain_on_undeclared_still_returns_null() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${missing_var?.blah}".into()),
            &empty_ctx(),
        )
        .expect("undeclared identifier with ?. must not throw");
    assert_eq!(out, Value::Null);
}

/// `??` when the LHS is a declared-and-null value. Baseline JS
/// semantics: `null ?? '123'` → `'123'`.
#[test]
fn nullish_coalescing_on_declared_null_returns_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(&Value::String("${null ?? 'fb'}".into()), &empty_ctx())
        .expect("must not throw");
    assert_eq!(out, Value::String("fb".into()));
}

/// `??` when the LHS is a defined non-nullish value. JS semantics:
/// `0 ?? 'fb'` → `0` (NOT the fallback — `0` is not nullish).
#[test]
fn nullish_coalescing_on_zero_returns_zero_not_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(&Value::String("${0 ?? 'fb'}".into()), &empty_ctx())
        .expect("must not throw");
    // Value::Number for 0
    assert_eq!(out.as_i64(), Some(0));
}

/// Multiple undeclared identifiers in one expression: `${a || b}`
/// where both a, b are undeclared. Retry loop should declare each
/// as needed. `undefined || undefined` → `undefined` → `Value::Null`.
#[test]
fn multiple_undeclared_identifiers_in_one_expression_are_handled() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${aaa_undef || bbb_undef}".into()),
            &empty_ctx(),
        )
        .expect("must not throw even with two undeclared identifiers");
    assert_eq!(out, Value::Null);
}

/// TypeError from declared-but-null.foo must still surface. This is
/// the sharp edge of #57 — the ReferenceError swallow must NOT
/// extend to TypeError. Regression guard.
#[test]
fn typeerror_on_null_deref_still_surfaces() {
    let engine = ScriptEngine::new();
    let _err = engine
        .evaluate(
            &Value::String("${(function(){var f=null; return f.bar;})()}".into()),
            &empty_ctx(),
        )
        .expect_err("TypeError on null-deref must still fail; retry-with-declaration must NOT extend to TypeError");
}

/// Chained optional access + nullish: `${a?.b?.c ?? 'default'}`
/// where `a` is undeclared. Should return `'default'`.
#[test]
fn deep_optional_chain_with_nullish_fallback_returns_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${deep_missing?.b?.c ?? 'default'}".into()),
            &empty_ctx(),
        )
        .expect("must not throw");
    assert_eq!(out, Value::String("default".into()));
}

/// `||` (all-falsy fallback, distinct from `??` which only catches
/// nullish). Undeclared identifier → undefined → falsy → falls
/// through to the second operand.
#[test]
fn logical_or_on_undeclared_returns_second_operand() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${some_missing_var || 'fallback'}".into()),
            &empty_ctx(),
        )
        .expect("must not throw");
    assert_eq!(out, Value::String("fallback".into()));
}

/// `||` where LHS is a declared falsy value (`0`) — must return the
/// fallback. `||` catches ALL falsy, unlike `??` which only catches
/// nullish. JS parity — this is the key distinction between the two.
#[test]
fn logical_or_on_zero_returns_fallback_but_nullish_returns_zero() {
    let engine = ScriptEngine::new();
    // `||`: 0 is falsy → fallback fires.
    let or_out = engine
        .evaluate(&Value::String("${0 || 'fb'}".into()), &empty_ctx())
        .expect("must not throw");
    assert_eq!(or_out, Value::String("fb".into()), "|| on 0 → 'fb'");

    // `??`: 0 is NOT nullish → 0 wins.
    let nullish_out = engine
        .evaluate(&Value::String("${0 ?? 'fb'}".into()), &empty_ctx())
        .expect("must not throw");
    assert_eq!(nullish_out.as_i64(), Some(0), "?? on 0 → 0");
}

/// Mixed-string interpolation: `"prefix ${undeclared ?? 'fb'} suffix"`.
/// The `${…}` segment inside a mixed string still gets full retry
/// treatment, so `??` fires correctly and the segment renders as `'fb'`.
/// Guards against a regression where the mixed-string code path
/// bypasses the retry loop.
#[test]
fn mixed_string_with_nullish_on_undeclared_renders_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("prefix ${some_missing_id ?? 'fb'} suffix".into()),
            &empty_ctx(),
        )
        .expect("must not throw");
    assert_eq!(out, Value::String("prefix fb suffix".into()));
}

/// Chained `??`: multiple fallbacks in a row. `${a ?? b ?? 'last'}`
/// where a and b are both undeclared should walk the chain and land
/// on `'last'`. Guards against a code path where only the first
/// undeclared identifier is declared and the second still throws.
#[test]
fn chained_nullish_walks_all_fallbacks_when_all_lhs_undeclared() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${first_missing ?? second_missing ?? 'final'}".into()),
            &empty_ctx(),
        )
        .expect("must not throw across multiple undeclared identifiers");
    assert_eq!(out, Value::String("final".into()));
}

/// Nested optional chain across two undeclared identifiers:
/// `${a?.b(c)?.d ?? 'fb'}` — the method call inside the optional
/// chain references another undeclared identifier `c`. Both get
/// declared as undefined; the `?.` shorts the chain; `??` picks the
/// fallback.
#[test]
fn optional_chain_with_call_referencing_undeclared_returns_fallback() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${outer?.inner(inner_missing)?.tail ?? 'fb'}".into()),
            &empty_ctx(),
        )
        .expect("must not throw");
    assert_eq!(out, Value::String("fb".into()));
}

/// Retry cap: an expression that references more than
/// MAX_UNDECLARED_RETRIES distinct undeclared identifiers must fail
/// cleanly with a descriptive error rather than loop forever. The
/// cap is 16 — build an expression with 20 distinct identifiers to
/// exceed it. `undefined + undefined + ...` sums to NaN in JS, but
/// each identifier needs its own retry to reach that point.
#[test]
fn expression_exceeding_retry_cap_fails_cleanly() {
    let engine = ScriptEngine::new();
    // Twenty distinct undeclared identifiers, summed.
    let expr = format!(
        "${{{}}}",
        (0..20)
            .map(|i| format!("u{}", i))
            .collect::<Vec<_>>()
            .join(" + ")
    );
    let err = engine
        .evaluate(&Value::String(expr), &empty_ctx())
        .expect_err("must fail — exceeds retry cap");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("retry")
            || msg.contains("undeclared")
            || msg.contains("cap"),
        "cap-exceeded error must mention the cap for operator visibility: {msg}"
    );
}

/// Regression: `?.` on a declared-but-undefined value must short-
/// circuit cleanly, NOT trigger a retry. Retries only fire on
/// ReferenceError, not on `undefined` values.
///
/// `${(void 0)?.foo ?? 'fb'}` — `void 0` yields undefined; `?.`
/// shorts; `??` picks fallback. No retry involved.
#[test]
fn optional_chain_on_declared_undefined_does_not_retry() {
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${(void 0)?.foo ?? 'fb'}".into()),
            &empty_ctx(),
        )
        .expect("must not throw");
    assert_eq!(out, Value::String("fb".into()));
}

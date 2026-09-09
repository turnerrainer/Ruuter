//! Issue #90 — empirical verification of the JavaScript subset
//! supported inside `${…}` DSL expressions.
//!
//! Runs a matrix of expression → expected-value pairs against the
//! active `ScriptEngine` (Boa by default; QuickJS under
//! `--no-default-features --features scripting-quickjs`). Doubles as:
//!
//! - **Regression pin**: any listed row that stops working on a
//!   future engine upgrade fails this test.
//! - **Coverage anchor**: adding a new row exercises the same
//!   construct on both backends the release gate exercises.
//! - **Source of truth for `book/src/dsl/expressions.md`**: the
//!   documented "supported" list is exactly what this file
//!   verifies.
//!
//! Constructs that DIVERGE between Boa and QuickJS live in their
//! own `#[cfg(feature = ...)]`-gated block at the bottom so the
//! primary matrix stays backend-agnostic.

use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::scripting::ScriptEngine;
use serde_json::{json, Value};
use std::collections::HashMap;

fn empty_ctx() -> ExecutionContext {
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
}

/// Evaluate a whole-string `${expr}` under the active engine.
/// Returns the Value on success; panics with a helpful diagnostic
/// on error (so a failing row names which construct broke).
fn eval(expr: &str) -> Value {
    let engine = ScriptEngine::new();
    match engine.evaluate(&Value::String(format!("${{{expr}}}")), &empty_ctx()) {
        Ok(v) => v,
        Err(e) => panic!("expression `${{{expr}}}` failed to evaluate: {e}"),
    }
}

/// Assert `${expr}` evaluates to `expected`.
fn check(expr: &str, expected: Value) {
    let actual = eval(expr);
    assert_eq!(
        actual, expected,
        "expression `${{{expr}}}` produced unexpected value"
    );
}

// ────────────────────────────────────────────────────────────────
// Core primitives — both backends
// ────────────────────────────────────────────────────────────────

#[test]
fn arithmetic_and_comparisons() {
    check("1 + 2", json!(3));
    check("10 - 3", json!(7));
    check("4 * 5", json!(20));
    check("10 / 2", json!(5));
    check("7 % 3", json!(1));
    check("2 ** 10", json!(1024));
    check("1 < 2", json!(true));
    check("1 > 2", json!(false));
    check("1 === 1", json!(true));
    check("1 === '1'", json!(false));
    check("1 == '1'", json!(true));
    check("1 !== '1'", json!(true));
}

#[test]
fn logical_and_nullish() {
    check("true && false", json!(false));
    check("true || false", json!(true));
    check("!true", json!(false));
    check("null ?? 'fallback'", json!("fallback"));
    check("undefined ?? 'fallback'", json!("fallback"));
    check("'value' ?? 'fallback'", json!("value"));
    check("false ?? 'fallback'", json!(false));
    check("0 ?? 'fallback'", json!(0));
}

#[test]
fn optional_chaining() {
    check("({a:{b:1}}).a?.b", json!(1));
    check("({a:null}).a?.b", Value::Null);
    // Optional-chain on a function call.
    check("({f: () => 42}).f?.()", json!(42));
    check("({}).f?.()", Value::Null);
}

#[test]
fn ternary() {
    check("true ? 'yes' : 'no'", json!("yes"));
    check("false ? 'yes' : 'no'", json!("no"));
    // Whole-scalar ternary — the very shape that trips YAML plain-
    // scalar parsing (see issue #91 / book/src/dsl/yaml-gotchas.md);
    // inside this test the expression is a Rust string literal so
    // YAML isn't in the picture.
    check("(1 === 1) ? 42 : 99", json!(42));
}

#[test]
fn typeof_operator() {
    check("typeof 'x'", json!("string"));
    check("typeof 1", json!("number"));
    check("typeof true", json!("boolean"));
    check("typeof null", json!("object"));
    check("typeof undefined", json!("undefined"));
    check("typeof []", json!("object"));
    check("typeof {}", json!("object"));
}

// ────────────────────────────────────────────────────────────────
// String methods
// ────────────────────────────────────────────────────────────────

#[test]
fn string_length_and_case() {
    check("'hello'.length", json!(5));
    check("'HELLO'.toLowerCase()", json!("hello"));
    check("'hello'.toUpperCase()", json!("HELLO"));
}

#[test]
fn string_starts_ends_includes() {
    check("'hello'.startsWith('he')", json!(true));
    check("'hello'.startsWith('lo')", json!(false));
    check("'hello'.endsWith('lo')", json!(true));
    check("'hello'.endsWith('he')", json!(false));
    check("'hello'.includes('ell')", json!(true));
    check("'hello'.includes('xy')", json!(false));
}

#[test]
fn string_slice_substring() {
    check("'hello'.substring(1, 4)", json!("ell"));
    check("'hello'.slice(1, 4)", json!("ell"));
    check("'hello'.slice(-2)", json!("lo"));
}

#[test]
fn string_split_and_index() {
    check("'a,b,c'.split(',')", json!(["a", "b", "c"]));
    check("'hello'.indexOf('l')", json!(2));
    check("'hello'.lastIndexOf('l')", json!(3));
    check("'hello'.charAt(1)", json!("e"));
    check("'hello'.charCodeAt(1)", json!(101));
}

#[test]
fn string_trim() {
    check("'  hi  '.trim()", json!("hi"));
    check("'  hi  '.trimStart()", json!("hi  "));
    check("'  hi  '.trimEnd()", json!("  hi"));
}

#[test]
fn string_replace() {
    check("'hello'.replace('l', 'L')", json!("heLlo"));
    // `replaceAll` is ES2021; both backends should support it in
    // 2026-era versions. Row exists so a regression is loud.
    check("'hello'.replaceAll('l', 'L')", json!("heLLo"));
}

#[test]
fn string_conversion() {
    check("String(42)", json!("42"));
    check("String(true)", json!("true"));
    check("String(null)", json!("null"));
    check("(42).toString()", json!("42"));
    check("true.toString()", json!("true"));
}

// ────────────────────────────────────────────────────────────────
// Array methods
// ────────────────────────────────────────────────────────────────

#[test]
fn array_isarray_and_length() {
    check("Array.isArray([1,2,3])", json!(true));
    check("Array.isArray('nope')", json!(false));
    check("Array.isArray({})", json!(false));
    check("[1,2,3].length", json!(3));
}

#[test]
fn array_higher_order() {
    check("[1,2,3].map(x => x * 2)", json!([2, 4, 6]));
    check("[1,2,3].filter(x => x > 1)", json!([2, 3]));
    check("[1,2,3].some(x => x > 2)", json!(true));
    check("[1,2,3].every(x => x > 0)", json!(true));
    check("[1,2,3].find(x => x > 1)", json!(2));
    check("[1,2,3].findIndex(x => x > 1)", json!(1));
    check("[1,2,3].reduce((a, b) => a + b, 0)", json!(6));
}

#[test]
fn array_join_concat_includes() {
    check("[1,2,3].join('-')", json!("1-2-3"));
    check("[1,2].concat([3,4])", json!([1, 2, 3, 4]));
    check("[1,2,3].includes(2)", json!(true));
    check("[1,2,3].includes(9)", json!(false));
    check("[1,2,3].indexOf(2)", json!(1));
}

#[test]
fn array_spread() {
    check("[...[1,2], 3, ...[4,5]]", json!([1, 2, 3, 4, 5]));
}

// ────────────────────────────────────────────────────────────────
// Object methods
// ────────────────────────────────────────────────────────────────

#[test]
fn object_reflection() {
    check("Object.keys({a:1, b:2})", json!(["a", "b"]));
    check("Object.values({a:1, b:2})", json!([1, 2]));
    check("Object.entries({a:1})", json!([["a", 1]]));
}

#[test]
fn object_assign_and_spread() {
    check("Object.assign({}, {a:1}, {b:2})", json!({"a":1, "b":2}));
    check("({...({a:1}), b:2})", json!({"a":1, "b":2}));
}

// ────────────────────────────────────────────────────────────────
// Conversion
// ────────────────────────────────────────────────────────────────

#[test]
fn number_and_boolean_coercion() {
    check("Number('42')", json!(42));
    check("Number('42.5')", json!(42.5));
    check("Boolean(0)", json!(false));
    check("Boolean(1)", json!(true));
    check("Boolean('')", json!(false));
    check("Boolean('x')", json!(true));
    check("parseInt('42', 10)", json!(42));
    check("parseFloat('42.5')", json!(42.5));
}

// ────────────────────────────────────────────────────────────────
// JSON
// ────────────────────────────────────────────────────────────────

#[test]
fn json_parse_and_stringify() {
    check("JSON.parse('{\"a\":1}')", json!({"a": 1}));
    check("JSON.stringify({a:1})", json!("{\"a\":1}"));
    check("JSON.parse('[1,2,3]')", json!([1, 2, 3]));
}

// ────────────────────────────────────────────────────────────────
// Math
// ────────────────────────────────────────────────────────────────

#[test]
fn math_helpers() {
    check("Math.floor(3.7)", json!(3));
    check("Math.ceil(3.2)", json!(4));
    check("Math.round(3.5)", json!(4));
    check("Math.abs(-5)", json!(5));
    check("Math.min(1, 2, 3)", json!(1));
    check("Math.max(1, 2, 3)", json!(3));
    // Math.random() is nondeterministic — pin only the type.
    let r = eval("Math.random()");
    assert!(
        matches!(r, Value::Number(_)),
        "Math.random must return a number: {r:?}"
    );
}

// ────────────────────────────────────────────────────────────────
// Functions
// ────────────────────────────────────────────────────────────────

#[test]
fn arrow_and_iife() {
    check("((x) => x + 1)(41)", json!(42));
    check("(function(x){ return x + 1; })(41)", json!(42));
    // Nested arrows.
    check(
        "[[1,2],[3,4]].map(a => a.reduce((x,y) => x+y, 0))",
        json!([3, 7]),
    );
}

// ────────────────────────────────────────────────────────────────
// Regex — same shape on both backends (2026-era engine coverage)
// ────────────────────────────────────────────────────────────────

#[test]
fn regex_literal_and_methods() {
    check("/^he/.test('hello')", json!(true));
    check("/^lo/.test('hello')", json!(false));
    // .match returns null on no-match. Both backends surface that
    // as JSON null.
    check("'hello'.match(/xy/)", Value::Null);
    // A successful match returns an array; JSON-stringify normalises
    // the shape (the array carries `index` and `input` properties
    // that are engine-specific — we only care about the matched
    // portion here).
    let m = eval("'hello world'.match(/(\\w+) (\\w+)/)[0]");
    assert_eq!(m, json!("hello world"));
}

#[test]
fn regex_constructor() {
    check("new RegExp('^he').test('hello')", json!(true));
    check("new RegExp('lo$').test('hello')", json!(true));
}

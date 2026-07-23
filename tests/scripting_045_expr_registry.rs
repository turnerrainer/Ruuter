//! Task 045 — pre-parsed expression registry, lazy-compile per
//! session slot.
//!
//! Assertions written to BREAK a broken implementation:
//! - Registry extracts every `${...}` from a synthetic DSL tree,
//!   deduplicates identical sources, ordered by insertion.
//! - Two evaluates of the SAME expression on one context produce
//!   the same result byte-identically (cache doesn't corrupt on
//!   second use).
//! - Two evaluates of DIFFERENT expressions each produce their
//!   own result (no cross-contamination between slots).
//! - An unregistered expression (synthesised at runtime, not seen
//!   at DSL load) still evaluates correctly via the fallback
//!   path.

#![cfg(feature = "scripting-quickjs")]

use indexmap::IndexMap;
use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::dsl::loader::HttpDsls;
use ruuter_on_rust::dsl::Dsl;
use ruuter_on_rust::scripting::registry::Builder;
use ruuter_on_rust::scripting::ScriptEngine;
use ruuter_on_rust::steps::{AssignStep, DslStep, ReturnStep};
use serde_json::{json, Value};
use std::collections::HashMap;

fn ctx_with_registry_from(dsls: &HttpDsls) -> ExecutionContext {
    let mut b = Builder::new();
    b.add_http(dsls);
    let registry = b.freeze();
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
    .with_expr_registry(registry)
}

fn tiny_dsl_with_exprs(exprs: &[&str]) -> Dsl {
    // Synthetic DSL: one assign step per expression, then a return.
    // Enough surface for the registry walker to enumerate all
    // expressions verbatim.
    let mut steps: IndexMap<String, DslStep> = IndexMap::new();
    for (i, expr) in exprs.iter().enumerate() {
        let name = format!("step_{}", i);
        let mut assign_map = HashMap::new();
        assign_map.insert(format!("v{}", i), Value::String(format!("${{{}}}", expr)));
        steps.insert(
            name,
            DslStep::Assign(AssignStep {
                assign: assign_map,
                next: None,
                skip: None,
            }),
        );
    }
    steps.insert(
        "respond".into(),
        DslStep::Return(ReturnStep {
            return_value: json!({"ok": true}),
            status: None,
            headers: None,
            wrapper: None,
            next: None,
        }),
    );
    Dsl::new(steps)
}

fn dsls_from(dsl: Dsl) -> HttpDsls {
    let mut paths = HashMap::new();
    paths.insert("test_path".to_string(), dsl);
    let mut methods = HashMap::new();
    methods.insert("GET".to_string(), paths);
    let mut http = HashMap::new();
    http.insert("test_proj".to_string(), methods);
    http
}

// ── Registry contents ────────────────────────────────────────────

#[test]
fn registry_captures_expressions_from_dsl_tree() {
    let dsl = tiny_dsl_with_exprs(&["1 + 1", "'a' + 'b'", "42"]);
    let dsls = dsls_from(dsl);
    let mut b = Builder::new();
    b.add_http(&dsls);
    let r = b.freeze();

    assert_eq!(r.len(), 3);
    assert_eq!(r.id_for("1 + 1"), Some(0));
    assert_eq!(r.id_for("'a' + 'b'"), Some(1));
    assert_eq!(r.id_for("42"), Some(2));
    assert_eq!(r.id_for("never seen"), None);
}

#[test]
fn registry_deduplicates_identical_sources() {
    let dsl = tiny_dsl_with_exprs(&["x + 1", "y * 2", "x + 1"]);
    let dsls = dsls_from(dsl);
    let mut b = Builder::new();
    b.add_http(&dsls);
    let r = b.freeze();
    assert_eq!(r.len(), 2, "duplicate expression source must share one id");
}

// ── Runtime correctness on the cached path ───────────────────────

#[test]
fn cached_expression_returns_correct_value_on_first_and_second_call() {
    let dsl = tiny_dsl_with_exprs(&["10 + 20", "'foo' + 'bar'"]);
    let dsls = dsls_from(dsl);
    let engine = ScriptEngine::new();
    let ctx = ctx_with_registry_from(&dsls);

    // First eval — compile+invoke path
    let out1 = engine.evaluate(&json!("${10 + 20}"), &ctx).unwrap();
    assert_eq!(out1, json!(30));

    // Second eval of the SAME expression — invoke-only path
    let out2 = engine.evaluate(&json!("${10 + 20}"), &ctx).unwrap();
    assert_eq!(out2, json!(30));

    // First eval of a DIFFERENT expression — compile+invoke path
    let out3 = engine.evaluate(&json!("${'foo' + 'bar'}"), &ctx).unwrap();
    assert_eq!(out3, json!("foobar"));
}

#[test]
fn cached_expressions_do_not_cross_contaminate() {
    // Two expressions that produce different values. Registered
    // and cached in the same session — the id-based dispatch must
    // route each to its own compiled function.
    let dsl = tiny_dsl_with_exprs(&["100 * 2", "100 * 3"]);
    let dsls = dsls_from(dsl);
    let engine = ScriptEngine::new();
    let ctx = ctx_with_registry_from(&dsls);

    // Warm both slots
    let _ = engine.evaluate(&json!("${100 * 2}"), &ctx).unwrap();
    let _ = engine.evaluate(&json!("${100 * 3}"), &ctx).unwrap();

    // Second call each — must return the RIGHT cached value.
    assert_eq!(
        engine.evaluate(&json!("${100 * 2}"), &ctx).unwrap(),
        json!(200)
    );
    assert_eq!(
        engine.evaluate(&json!("${100 * 3}"), &ctx).unwrap(),
        json!(300)
    );
}

// ── Unregistered expression fallback ─────────────────────────────

#[test]
fn unregistered_expression_falls_back_to_per_eval_compile() {
    // Context built with an EMPTY registry — no expressions were
    // known at "load time". The evaluate path must still work,
    // hitting the fallback compile-inline branch.
    let engine = ScriptEngine::new();
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    // No with_expr_registry() — registry is the default empty one.
    let out = engine.evaluate(&json!("${5 * 7}"), &ctx).unwrap();
    assert_eq!(out, json!(35));
}

// ── Incoming binding still works through the cached path ────────

#[test]
fn cached_expression_sees_refreshed_bindings_between_calls() {
    // Bindings (incoming.body, user vars) are refreshed at the
    // start of every evaluate. Cached function references them
    // dynamically via JS closure over globalThis, so the SECOND
    // eval must see updated bindings, not stale values.
    let dsl = tiny_dsl_with_exprs(&["incoming.body.n * 2"]);
    let dsls = dsls_from(dsl);
    let engine = ScriptEngine::new();
    // Build TWO contexts to simulate two requests sharing DSL
    // — but same registry, so same ids.
    let mut b = Builder::new();
    b.add_http(&dsls);
    let registry = b.freeze();

    let mut body1 = HashMap::new();
    body1.insert("n".to_string(), json!(3));
    let ctx1 = ExecutionContext::new(body1, HashMap::new(), HashMap::new(), "test".into())
        .with_expr_registry(registry.clone());
    let out1 = engine
        .evaluate(&json!("${incoming.body.n * 2}"), &ctx1)
        .unwrap();
    assert_eq!(out1, json!(6));

    let mut body2 = HashMap::new();
    body2.insert("n".to_string(), json!(50));
    let ctx2 = ExecutionContext::new(body2, HashMap::new(), HashMap::new(), "test".into())
        .with_expr_registry(registry);
    let out2 = engine
        .evaluate(&json!("${incoming.body.n * 2}"), &ctx2)
        .unwrap();
    assert_eq!(
        out2,
        json!(100),
        "second context must see its own body, not the first's"
    );
}

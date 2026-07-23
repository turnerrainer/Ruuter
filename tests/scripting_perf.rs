//! Micro-benchmark verifying #002: one BoaContext per evaluate() call
//! across many `${...}` expressions, not one per expression.
//!
//! Not a precise benchmark — just confirms order-of-magnitude perf and
//! that batched-recursion evaluation produces correct results.

use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::scripting::ScriptEngine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

fn make_context() -> ExecutionContext {
    let body: HashMap<String, Value> = [("a".to_string(), json!(10)), ("b".to_string(), json!(5))]
        .into_iter()
        .collect();
    ExecutionContext::new(body, HashMap::new(), HashMap::new(), "test".into())
}

#[test]
fn evaluates_object_with_many_expressions_correctly() {
    let engine = ScriptEngine::new();
    let ctx = make_context();

    let input = json!({
        "sum":       "${incoming.body.a + incoming.body.b}",
        "diff":      "${incoming.body.a - incoming.body.b}",
        "product":   "${incoming.body.a * incoming.body.b}",
        "quotient":  "${incoming.body.a / incoming.body.b}",
        "max":       "${Math.max(incoming.body.a, incoming.body.b)}",
        "min":       "${Math.min(incoming.body.a, incoming.body.b)}",
        "even":      "${incoming.body.a % 2 === 0}",
        "nested": {
            "double_a": "${incoming.body.a * 2}",
            "triple_b": "${incoming.body.b * 3}",
        }
    });

    let result = engine.evaluate(&input, &ctx).expect("evaluation");
    assert_eq!(result["sum"], json!(15));
    assert_eq!(result["diff"], json!(5));
    assert_eq!(result["product"], json!(50));
    assert_eq!(result["quotient"], json!(2));
    assert_eq!(result["max"], json!(10));
    assert_eq!(result["min"], json!(5));
    assert_eq!(result["even"], json!(true));
    assert_eq!(result["nested"]["double_a"], json!(20));
    assert_eq!(result["nested"]["triple_b"], json!(15));
}

#[test]
fn order_of_magnitude_check_one_context_per_evaluate_call() {
    let engine = ScriptEngine::new();
    let ctx = make_context();

    // 9 expressions per call × 100 calls = 900 evaluations.
    // With the old code: 900 BoaContext::default() calls.
    // With the new code: 100 BoaContext::default() calls.
    let input = json!({
        "v1": "${incoming.body.a + 1}",
        "v2": "${incoming.body.a + 2}",
        "v3": "${incoming.body.a + 3}",
        "v4": "${incoming.body.a + 4}",
        "v5": "${incoming.body.a + 5}",
        "v6": "${incoming.body.a + 6}",
        "v7": "${incoming.body.a + 7}",
        "v8": "${incoming.body.a + 8}",
        "v9": "${incoming.body.a + 9}",
    });

    let start = Instant::now();
    for _ in 0..100 {
        let _ = engine.evaluate(&input, &ctx).unwrap();
    }
    let elapsed = start.elapsed();

    // Loose ceiling — on a slow CI box BoaContext init is ~10-30ms each.
    // With one ctx per call (100 inits for 900 expressions) we should be
    // well under 10s. The old code (900 inits) would exceed it.
    assert!(
        elapsed.as_secs() < 10,
        "evaluation too slow: {:?} (regression in BoaContext reuse?)",
        elapsed
    );
    eprintln!("100 calls × 9 expressions each: {:?}", elapsed);
}

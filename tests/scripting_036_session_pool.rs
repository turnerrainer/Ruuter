//! Task 036 — per-request QuickJS session pool.
//!
//! Assertions written to BREAK a broken implementation:
//! - First `evaluate()` on a context builds a session; the
//!   `boa_context_created_count()` metric bumps by exactly 1.
//! - Second `evaluate()` on the SAME context reuses the session
//!   — the metric does NOT bump again.
//! - Two independent ExecutionContexts get independent sessions
//!   (metric bumps twice).
//! - Cloning an ExecutionContext preserves the SAME session
//!   (metric doesn't bump on the clone's first evaluate).
//!
//! Boa build: the assertions all become "engine created per
//! evaluate" — Boa doesn't have a session pool. Gate the file
//! behind the QuickJS feature so it compiles to nothing on Boa.

#![cfg(feature = "scripting-quickjs")]

use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::scripting::{boa_context_created_count, ScriptEngine};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

/// The `boa_context_created_count()` counter is process-global.
/// Cargo runs tests in parallel by default; other tests would bump
/// the counter between our `before`/`after` snapshots and inflate
/// deltas. Serialise this test-file's tests via a file-scoped mutex
/// so each holds the counter exclusive for its measurement window.
///
/// This mutex only guards THIS file's tests. Tests in OTHER files
/// (e.g. scripting_037_literal_fastpath.rs) can still run in
/// parallel — they don't measure delta counts, they use
/// `evaluate_tracked`'s per-call return value which is race-free.
static COUNTER_LOCK: Mutex<()> = Mutex::new(());

fn fresh_ctx() -> ExecutionContext {
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
}

#[test]
fn second_evaluate_on_same_context_reuses_session() {
    let engine = ScriptEngine::new();
    let ctx = fresh_ctx();

    let _guard = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let before = boa_context_created_count();

    let (_, boa_used_1) = engine
        .evaluate_tracked(&json!("${1 + 1}"), &ctx)
        .expect("first eval");
    assert!(boa_used_1, "first eval must invoke the engine");
    let after_first = boa_context_created_count();
    assert_eq!(
        after_first - before,
        1,
        "first eval must construct exactly one session"
    );

    // Same context — the pool holds the session in a shared
    // OnceLock, so the second evaluate must reuse it.
    let (_, boa_used_2) = engine
        .evaluate_tracked(&json!("${2 + 2}"), &ctx)
        .expect("second eval");
    assert!(boa_used_2, "second eval also invokes the engine (different expr)");
    let after_second = boa_context_created_count();
    assert_eq!(
        after_second - after_first,
        0,
        "second eval on the SAME context must reuse the session — got {} new constructions",
        after_second - after_first
    );
}

#[test]
fn independent_contexts_get_independent_sessions() {
    let engine = ScriptEngine::new();
    let ctx1 = fresh_ctx();
    let ctx2 = fresh_ctx();

    let _guard = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let before = boa_context_created_count();
    let _ = engine.evaluate_tracked(&json!("${1}"), &ctx1).unwrap();
    let _ = engine.evaluate_tracked(&json!("${2}"), &ctx2).unwrap();
    let after = boa_context_created_count();

    assert_eq!(
        after - before,
        2,
        "two distinct contexts must produce two session constructions"
    );
}

#[test]
fn cloned_context_shares_session() {
    // ExecutionContext is Clone — sub-steps like iterate.do and
    // template step get clones. Task 036's design has all clones
    // share the same Arc<OnceLock<QuickJsSession>>, so a clone
    // seeing the first eval must reuse.
    let engine = ScriptEngine::new();
    let parent = fresh_ctx();

    let _guard = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let before = boa_context_created_count();
    let _ = engine.evaluate_tracked(&json!("${1 + 1}"), &parent).unwrap();
    let after_first = boa_context_created_count();
    assert_eq!(after_first - before, 1);

    let child = parent.clone();
    let _ = engine.evaluate_tracked(&json!("${2 + 2}"), &child).unwrap();
    let after_child = boa_context_created_count();
    assert_eq!(
        after_child - after_first,
        0,
        "cloned context must share the parent's session — got {} new",
        after_child - after_first
    );
}

#[test]
fn literal_fastpath_does_not_construct_session() {
    // Task 037 fast-path — expression-free values bypass the
    // engine entirely. This must hold on the QuickJS backend too:
    // no session ever gets built for a pure literal.
    let engine = ScriptEngine::new();
    let ctx = fresh_ctx();

    let _guard = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let before = boa_context_created_count();
    let (out, boa_used) = engine
        .evaluate_tracked(&json!({"static": "no expression"}), &ctx)
        .unwrap();
    let after = boa_context_created_count();

    assert!(!boa_used, "037 fast-path must skip engine invocation");
    assert_eq!(
        after - before,
        0,
        "no session should be built for a literal-only value"
    );
    assert_eq!(out, json!({"static": "no expression"}));
}

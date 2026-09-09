//! Issue #79 — `template:` step guards recurse infinitely, killing
//! the worker thread with a stack overflow.
//!
//! Reporter (sviljus): a project-wide `.guard.yml` that delegates its
//! auth check to `template: helpers/check-user-authority` looks fine.
//! Every request that reaches the guard's template step aborts the
//! tokio worker with `fatal runtime error: stack overflow`. Container
//! exits 134. Introduced by v0.9.11-rc H1 (PR #72 — template step now
//! enforces guards on its target). The recursion:
//!
//!   HTTP entry → run project guard
//!     project guard → template step
//!       template → applicable_guards_for(target) → [project guard]
//!         → run project guard (again, same key)
//!           → template step (again) → …
//!
//! Fix: track guard keys currently mid-execution on the ExecutionContext,
//! filter `applicable_guards_for` output against that set, and hard-cap
//! nested depth as belt-and-braces. See `src/context/mod.rs::push_guard`.
//!
//! Every test in this file is written so that pre-fix it would either
//! panic (stack overflow) or produce the wrong status; post-fix it
//! settles on the intended shape.

#![allow(clippy::field_reassign_with_default)]

use arc_swap::ArcSwap;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::context::{ExecutionContext, MAX_GUARD_DEPTH};
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build(files: &[(&str, &str)]) -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-79-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared_http = Arc::new(ArcSwap::from_pointee(loaded.http));
    let shared_guards = Arc::new(ArcSwap::from_pointee(loaded.guards));
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(shared_http.clone())
        .with_guards(shared_guards.clone(), cfg.guards.mode);
    DslRouter::from_shared(
        shared_http,
        shared_guards,
        cfg,
        StateStore::new(),
        ws,
        engine,
    )
}

// ────────────────────────────────────────────────────────────────
// Integration — reporter's minimal repro
// ────────────────────────────────────────────────────────────────

/// Exact reproduction from the issue. Project-wide `svc/.guard.yml`
/// delegates its check to `template: helpers/noop` (which is itself
/// under the same project guard). Pre-fix: stack overflow, worker
/// aborts. Post-fix: 200 "pong".
#[tokio::test]
async fn reporter_repro_project_guard_delegates_to_template_under_same_guard() {
    let router = build(&[
        (
            "svc/.guard.yml",
            r#"
check:
  template: "helpers/noop"
  requestType: GET
  result: r
  next: allow

allow:
  status: 200
  return: "ok"
  next: end
"#,
        ),
        (
            "svc/GET/helpers/noop.yml",
            r#"
respond:
  return: "noop"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/ping.yml",
            r#"
respond:
  return: "pong"
  status: 200
  next: end
"#,
        ),
    ]);
    let outcome = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl must not stack-overflow");
    assert_eq!(
        outcome.status, 200,
        "route DSL should be reached: {outcome:?}"
    );
    let body = serde_json::to_string(&outcome.value).unwrap_or_default();
    assert!(body.contains("pong"), "expected 'pong' in body: {body}");
}

/// Same shape but the shared helper carries a non-trivial return — a
/// stand-in for the reporter's real-world "TIM userinfo + DB lookup"
/// template. Confirms the template result binds correctly under
/// `${r}` even when the same-guard filter fires.
#[tokio::test]
async fn guard_template_binds_return_value_even_when_same_guard_filtered() {
    let router = build(&[
        (
            "svc/.guard.yml",
            r#"
check:
  template: "helpers/whoami"
  requestType: GET
  result: r
  next: decide

decide:
  switch:
    - condition: "${r.userId == null}"
      next: deny
  next: allow

allow:
  status: 200
  return: "ok"
  next: end

deny:
  status: 401
  return: { error: "no user" }
  next: end
"#,
        ),
        (
            "svc/GET/helpers/whoami.yml",
            r#"
respond:
  return: { userId: 42, roles: ["viewer"] }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/ping.yml",
            r#"
respond:
  return: "pong"
  status: 200
  next: end
"#,
        ),
    ]);
    let outcome = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    assert_eq!(outcome.status, 200);
    let body = serde_json::to_string(&outcome.value).unwrap_or_default();
    assert!(
        body.contains("pong"),
        "route must run after guard admits: {body}"
    );
}

/// Guard denial still fires when the template result triggers the
/// guard's own deny branch. Confirms the same-guard filter doesn't
/// accidentally let deny logic through.
#[tokio::test]
async fn guard_can_still_deny_after_template_result_binds() {
    let router = build(&[
        (
            "svc/.guard.yml",
            r#"
check:
  template: "helpers/whoami"
  requestType: GET
  result: r
  next: decide

decide:
  switch:
    - condition: "${r.userId == null}"
      next: deny
  next: allow

allow:
  status: 200
  return: "ok"
  next: end

deny:
  status: 401
  return: { error: "no user" }
  next: end
"#,
        ),
        (
            "svc/GET/helpers/whoami.yml",
            r#"
respond:
  return: { userId: null, roles: [] }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/ping.yml",
            r#"
respond:
  return: "pong"
  status: 200
  next: end
"#,
        ),
    ]);
    let outcome = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    assert_eq!(
        outcome.status, 401,
        "guard's deny branch must fire: {outcome:?}"
    );
    let body = serde_json::to_string(&outcome.value).unwrap_or_default();
    assert!(body.contains("no user"), "deny body must surface: {body}");
    assert!(
        !body.contains("pong"),
        "route DSL must not run after guard 401: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// H1 preservation — non-recursive templates STILL run target guards
// ────────────────────────────────────────────────────────────────

/// H1 semantics untouched: a non-guard DSL that templates into a
/// guarded route still triggers the guard chain. Same test shape as
/// `security_h2ck_v0_9_10_rc::template_step_must_run_target_dsl_guards`
/// but scoped to issue #79 to catch regressions if the recursion
/// fix accidentally over-filters.
#[tokio::test]
async fn non_guard_dsl_still_triggers_target_guard_via_template() {
    let router = build(&[
        (
            "svc/POST/admin/.guard.yml",
            r#"
deny:
  status: 403
  return: { error: "admin guard denied" }
  next: end
"#,
        ),
        (
            "svc/POST/admin/things.yml",
            r#"
respond:
  return: { ok: true, reached_admin: true }
  status: 200
  next: end
"#,
        ),
        (
            "svc/POST/public/entry.yml",
            r#"
call_admin:
  template: admin/things
  request_type: POST
  result: r
  next: shape

shape:
  return: { proxied: "${r}" }
  next: end
"#,
        ),
    ]);
    let outcome = router
        .execute_dsl(
            "svc",
            "POST",
            "public/entry",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    let body = serde_json::to_string(&outcome.value).unwrap_or_default();
    assert!(
        !body.contains("reached_admin"),
        "H1 bypass regression: template reached admin body without running guard: {body}"
    );
    assert!(
        body.contains("admin guard denied"),
        "expected guard denial to bind via template result: {body}"
    );
}

/// The router's HTTP-entry guard loop pushes the guard's key onto the
/// stack so that any template step inside the guard sees it. If the
/// entry-point push were skipped, the reporter's repro would still
/// overflow. Verifies with two guards on the entry path: the outer
/// guard's template hits the target's guard chain, which includes
/// BOTH the outer and inner guards — the outer should be filtered
/// (already on the stack) and the inner should run once.
#[tokio::test]
async fn entry_point_guard_key_is_visible_to_inner_template_step() {
    let router = build(&[
        // Outer guard at project level.
        (
            "svc/.guard.yml",
            r#"
outer_check:
  template: "helpers/noop"
  requestType: GET
  result: outer_r
  next: allow

allow:
  status: 200
  return: "outer-ok"
  next: end
"#,
        ),
        // Inner guard on the whole GET tree — this guard runs both
        // at HTTP entry (as part of the terminal DSL's guard chain)
        // AND inside the outer's template call. In the second
        // invocation it should still fire (it isn't on the stack —
        // only the OUTER guard was pushed before its template ran).
        (
            "svc/GET/inner.guard.yml",
            r#"
inner_check:
  assign: { saw_inner: true }
  next: end
"#,
        ),
        (
            "svc/GET/helpers/noop.yml",
            r#"
respond:
  return: "noop"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/ping.yml",
            r#"
respond:
  return: "pong"
  status: 200
  next: end
"#,
        ),
    ]);
    let outcome = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    assert_eq!(outcome.status, 200);
    assert!(
        serde_json::to_string(&outcome.value)
            .unwrap_or_default()
            .contains("pong"),
        "route body must be reached: {outcome:?}"
    );
}

// ────────────────────────────────────────────────────────────────
// Unit — ExecutionContext::push_guard mechanics
// ────────────────────────────────────────────────────────────────

/// Baseline: fresh context has depth 0.
#[test]
fn fresh_context_has_empty_guard_stack() {
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    assert_eq!(ctx.guard_stack_depth(), 0);
    assert!(!ctx.is_guard_on_stack("anything"));
}

/// Push then drop restores depth.
#[test]
fn push_guard_returns_raii_guard_that_pops_on_drop() {
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    {
        let _frame = ctx.push_guard("A".into()).expect("push");
        assert_eq!(ctx.guard_stack_depth(), 1);
        assert!(ctx.is_guard_on_stack("A"));
    }
    assert_eq!(ctx.guard_stack_depth(), 0, "RAII pop must fire on drop");
    assert!(!ctx.is_guard_on_stack("A"));
}

/// Nested pushes stack correctly and each pop restores its own frame.
#[test]
fn nested_pushes_stack_and_pop_in_reverse() {
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    let a = ctx.push_guard("A".into()).unwrap();
    let b = ctx.push_guard("B".into()).unwrap();
    let c = ctx.push_guard("C".into()).unwrap();
    assert_eq!(ctx.guard_stack_depth(), 3);
    assert!(ctx.is_guard_on_stack("A"));
    assert!(ctx.is_guard_on_stack("B"));
    assert!(ctx.is_guard_on_stack("C"));
    drop(c);
    assert_eq!(ctx.guard_stack_depth(), 2);
    drop(b);
    assert_eq!(ctx.guard_stack_depth(), 1);
    drop(a);
    assert_eq!(ctx.guard_stack_depth(), 0);
}

/// Cycle detection: pushing a key that's already on the stack errors
/// with a clear diagnostic pointing at issue #79.
#[test]
fn cycle_push_returns_dsl_execution_error() {
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    let _outer = ctx.push_guard("A".into()).unwrap();
    let err = ctx
        .push_guard("A".into())
        .expect_err("double push must be rejected");
    let msg = format!("{err}");
    assert!(msg.contains("guard cycle detected"), "diagnostic: {msg}");
    assert!(msg.contains("'A'"), "must name the offending key: {msg}");
    assert!(msg.contains("issue #79"), "must cite the fix: {msg}");
}

/// Depth cap: pushing MAX_GUARD_DEPTH + 1 distinct keys errors with
/// a clear message. Belt-and-braces against exotic mutual-recursion
/// patterns that slip past the same-key cycle check.
#[test]
fn depth_cap_is_enforced_with_clear_error() {
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    let mut held = Vec::with_capacity(MAX_GUARD_DEPTH);
    for i in 0..MAX_GUARD_DEPTH {
        held.push(ctx.push_guard(format!("G{i}")).expect("within cap"));
    }
    assert_eq!(ctx.guard_stack_depth(), MAX_GUARD_DEPTH);
    let err = ctx
        .push_guard("overflow".into())
        .expect_err("beyond MAX_GUARD_DEPTH must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("MAX_GUARD_DEPTH"),
        "diagnostic names the cap: {msg}"
    );
    assert!(msg.contains("issue #79"));
}

/// `with_guard_stack_from` shares the same Arc — mutations on the
/// child are observable on the parent. This is the mechanism the
/// template step uses to make the enclosing guard's key visible to
/// nested guard chains.
#[test]
fn with_guard_stack_from_shares_the_arc_with_parent() {
    let parent = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    );
    let _outer = parent.push_guard("parent-guard".into()).unwrap();

    let child = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
    .with_guard_stack_from(&parent);

    // Child sees parent's frame.
    assert!(child.is_guard_on_stack("parent-guard"));
    assert_eq!(child.guard_stack_depth(), 1);

    // Push on the child is visible via the parent's view.
    let _inner = child.push_guard("child-guard".into()).unwrap();
    assert_eq!(parent.guard_stack_depth(), 2);
    assert!(parent.is_guard_on_stack("child-guard"));
}

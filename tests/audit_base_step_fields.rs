//! Audit finding 03 regression tests — Java-parity base step fields
//! (`skip`, `sleep`, `maxRecursions`) and the source-order fallthrough
//! semantics for missing `next:`.
//!
//! These tests would FAIL against the pre-fix code. They belong to
//! the finding-03 fix so a future regression that reintroduces the
//! `"end"` default or drops the base-field handling shows up here.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use tower::ServiceExt;

fn write_dsl(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router(dsl_root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

async fn hit(router: Arc<DslRouter>, path: &str) -> (u16, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.build_axum_router_from_arc().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// **Finding-03 core**: a step without an explicit `next:` MUST fall
/// through to the next step in source order. Pre-fix, every executor
/// hardcoded `Some("end")` when `next` was unset, so the second step
/// was never reached.
#[tokio::test]
async fn missing_next_falls_through_to_source_order() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hello.yml",
        r#"
step_a:
  assign:
    who: "world"
step_b:
  return: "hello ${who}"
  status: 200
"#,
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/hello").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"hello world\"}");
}

/// `skip: true` bypasses the step's action but STILL falls through
/// to source-order next (Java-parity). Pre-fix, `skip` was only
/// declared on `AssignStep` and even there was ignored by the
/// executor.
#[tokio::test]
async fn skip_true_bypasses_action_but_advances() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hello.yml",
        r#"
step_a:
  assign:
    who: "world"
  skip: true
step_b:
  return: "hello ${(typeof who === 'undefined' ? 'unassigned' : who)}"
  status: 200
"#,
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/hello").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"hello unassigned\"}");
}

/// `sleep: <ms>` blocks the step's dispatch for that many ms. We use
/// a small value (75 ms) to keep the test fast; the wall clock
/// comparison uses a generous 25 ms floor to survive scheduler
/// jitter but still catch "sleep field ignored" (which would be
/// well under 25 ms).
#[tokio::test]
async fn sleep_field_delays_step_dispatch() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hello.yml",
        r#"
step_slow:
  return: "delayed"
  status: 200
  sleep: 75
"#,
    );
    let started = Instant::now();
    let (status, _) = hit(build_router(tmp.path()), "/svc/hello").await;
    let elapsed = started.elapsed();
    assert_eq!(status, 200);
    assert!(
        elapsed.as_millis() >= 50,
        "sleep: 75 was ignored — request completed in {} ms",
        elapsed.as_millis()
    );
}

/// Per-step `maxRecursions` caps a loop step's ACTION count.
/// Pre-fix, `maxRecursions` was silently dropped by serde; the loop
/// ran to the global 10 000 cap and returned an empty response.
///
/// A well-formed DSL pairs `maxRecursions:` with a `switch:` that
/// notices the counter's ceiling and routes to a terminal step.
/// This test uses that pattern: after `maxRecursions: 3` hits on
/// `bump`, the engine stops executing `bump`; a follow-up `check`
/// sees `i == 3` and routes to `done`. Body reports the capped
/// count.
#[tokio::test]
async fn per_step_max_recursions_caps_loop() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/loop.yml",
        r#"
init:
  assign:
    i: 0
  next: bump
bump:
  assign:
    i: "${i + 1}"
  maxRecursions: 3
  next: check
check:
  switch:
    - condition: "${i >= 3}"
      next: done
    - condition: "${i < 3}"
      next: bump
done:
  return: { count: "${i}" }
  status: 200
"#,
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/loop").await;
    assert_eq!(status, 200);
    assert_eq!(
        body, "{\"response\":{\"count\":3}}",
        "bump.maxRecursions=3 should cap i at 3 and let done fire"
    );
}

/// Under the old code, a step with `next:` unset returned `"end"`,
/// which terminated the DSL. This test uses `switch` — same
/// no-condition-matched path — to prove Switch also falls through
/// now.
#[tokio::test]
async fn switch_with_no_match_falls_through_source_order() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hello.yml",
        r#"
gate:
  switch:
    - condition: "${false}"
      next: never
step_b:
  return: "reached"
  status: 200
never:
  return: "should not reach"
  status: 500
"#,
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/hello").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"reached\"}");
}

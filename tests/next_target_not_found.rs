//! Issue #61 — `next:` pointing to a non-existent step raises a
//! runtime error naming the source step and the missing target.
//! Previously the engine silently broke out of its loop and returned
//! an empty 200 response.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;

fn build_router(project: &str, method: &str, path: &str, body: &str) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-next-target-{}", uniq()));
    let dir = tmp.join(project).join(method);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{}.yml", path)), body).unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;

    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let dsls = loader.load_all().expect("load dsls");
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_logging(std::sync::Arc::new(cfg.logging.clone()))
        .with_ws_registry(ws_registry.clone());
    DslRouter::new(
        dsls,
        HashMap::new(),
        cfg,
        StateStore::new(),
        ws_registry,
        engine,
    )
}

fn uniq() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Reporter's core complaint: a typo in `next:` used to silently
/// yield an empty response. Now it raises an error naming both
/// the source step and the missing target.
#[tokio::test(flavor = "current_thread")]
async fn next_pointing_to_missing_step_is_a_runtime_error() {
    let dsl = r#"
setup:
  assign:
    x: 1
  next: rply

reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "typo", dsl);
    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "typo",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("must fail — next: target 'rply' does not exist");
    let msg = err.to_string();
    assert!(
        msg.contains("rply") && msg.contains("setup"),
        "error must name the missing target 'rply' AND the source step 'setup': {msg}"
    );
    assert!(
        msg.contains("no step named 'rply' exists"),
        "error must describe the missing-target condition plainly: {msg}"
    );
}

/// Same rule applies when the bad `next:` comes from a `switch`
/// condition, not a top-level step-level `next:`.
#[tokio::test(flavor = "current_thread")]
async fn switch_condition_pointing_to_missing_step_is_a_runtime_error() {
    let dsl = r#"
route:
  switch:
    - condition: "${true}"
      next: unknown_branch
  next: fallback

fallback:
  return: { picked: "fallback" }
  next: end
"#;
    let router = build_router("svc", "GET", "swtypo", dsl);
    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "swtypo",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("must fail — switch branch 'unknown_branch' does not exist");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown_branch") && msg.contains("route"),
        "error must name the missing target AND the source switch step: {msg}"
    );
}

/// `next: end` is the reserved terminator sentinel — MUST remain
/// valid and not trip the missing-target check.
#[tokio::test(flavor = "current_thread")]
async fn next_end_still_terminates_cleanly() {
    let dsl = r#"
reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "term", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "term",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("`next: end` must terminate cleanly");
    assert_eq!(res.value.unwrap()["ok"], true);
}

/// Regression baseline: a valid `next:` target still routes normally.
#[tokio::test(flavor = "current_thread")]
async fn next_pointing_to_declared_step_still_works() {
    let dsl = r#"
setup:
  assign:
    x: 1
  next: reply

reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "valid", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "valid",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("valid next: must route normally");
    assert_eq!(res.value.unwrap()["ok"], true);
}

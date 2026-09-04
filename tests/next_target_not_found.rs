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

/// Chained-jump case: step A → step B (valid) → step C where C
/// doesn't exist. The engine walks past A cleanly, executes B, then
/// hits the missing target from B. Error must name B as the source
/// and C as the target (NOT A, and NOT the whole chain).
#[tokio::test(flavor = "current_thread")]
async fn chained_jump_with_missing_target_names_the_jumping_step() {
    let dsl = r#"
alpha:
  assign:
    a: 1
  next: beta

beta:
  assign:
    b: 2
  next: gamma_but_missing

delta:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "chain", dsl);
    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "chain",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("must fail — beta's next: target doesn't exist");
    let msg = err.to_string();
    assert!(
        msg.contains("beta") && msg.contains("gamma_but_missing"),
        "error must name the immediate jumping step 'beta' and the missing target 'gamma_but_missing', NOT 'alpha': {msg}"
    );
    assert!(
        !msg.contains("alpha"),
        "error must NOT name earlier steps in the chain: {msg}"
    );
}

/// `http.<verb>` `error:` field pointing to a missing step. Under
/// audit finding 04 the http executor sets `next_step: Some(err_step)`
/// on non-allowed upstream status, which then flows through the same
/// engine lookup as any other `next:`. So the #61 fix covers this
/// path automatically — this test locks that in.
#[tokio::test(flavor = "current_thread")]
async fn http_error_pointing_to_missing_step_is_a_runtime_error() {
    use tempfile::TempDir;

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/fail")
        .with_status(500)
        .with_body("upstream oops")
        .with_header("content-type", "text/plain")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("svc").join("GET");
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("{}/fail", server.url());
    std::fs::write(
        dir.join("errjump.yml"),
        format!(
            r#"
try_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  error: no_such_handler
  next: unreachable

unreachable:
  return: "should not reach"
  next: end
"#
        ),
    )
    .unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp.path().to_path_buf();
    cfg.internal_requests.block_private_networks = false;
    // Reject anything above 299 so the mock's 500 trips the `error:` branch.
    cfg.http_codes_allow_list = vec![200, 201, 202, 204];

    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let dsls = loader.load_all().expect("load dsls");
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_logging(std::sync::Arc::new(cfg.logging.clone()))
        .with_ws_registry(ws_registry.clone());
    let router = DslRouter::new(
        dsls,
        HashMap::new(),
        cfg,
        StateStore::new(),
        ws_registry,
        engine,
    );

    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "errjump",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("http.error: to a missing step must raise the missing-target error");
    let msg = err.to_string();
    assert!(
        msg.contains("no_such_handler") && msg.contains("try_upstream"),
        "error must name the missing http.error: target and the http step as source: {msg}"
    );
}

/// `switch` with a fallthrough `next:` pointing to a missing step
/// (no branch matches, then `next: unknown_fallback` fires). Same
/// error path as a matched-branch missing target, tested separately
/// because the code path goes through the `no_match` sentinel first.
#[tokio::test(flavor = "current_thread")]
async fn switch_fallthrough_pointing_to_missing_step_is_a_runtime_error() {
    let dsl = r#"
route:
  switch:
    - condition: "${false}"
      next: never_taken
  next: unknown_fallback

never_taken:
  return: { picked: "never" }
  next: end
"#;
    let router = build_router("svc", "GET", "swfall", dsl);
    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "swfall",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("switch fallthrough to missing step must raise");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown_fallback") && msg.contains("route"),
        "error must name the missing fallthrough target AND the switch step: {msg}"
    );
}

/// Case-sensitivity guard: `next: Reply` when the step is `reply` is
/// a MISSING target, not a fuzzy match. Step names are compared as
/// literal strings — Java-parity.
#[tokio::test(flavor = "current_thread")]
async fn next_target_is_case_sensitive_and_a_case_mismatch_is_missing() {
    let dsl = r#"
setup:
  assign:
    x: 1
  next: Reply

reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "casing", dsl);
    let err = router
        .execute_dsl(
            "svc",
            "GET",
            "casing",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect_err("`Reply` (capital R) must not fuzzy-match `reply`");
    let msg = err.to_string();
    assert!(
        msg.contains("Reply"),
        "error must quote the exact requested target (case-preserved): {msg}"
    );
}

//! Issue #64 — switch conditions match on JS-truthy, not strict boolean.
//!
//! Java Ruuter uses `Boolean.TRUE.equals(...)` — a condition must
//! evaluate to literally `true`. Ruuter-on-Rust diverges (see
//! DIVERGENCES.md D-40): any JS-truthy value fires the branch.
//! Reporter's specific case:
//!
//! ```yaml
//! check_poll:
//!   switch:
//!     - condition: ${requestId && poll}     # both non-empty strings
//!       next: poll_remaining
//! ```
//!
//! `${requestId && poll}` returns the string `poll` (JS: `&&` returns
//! the second operand when the first is truthy). Under strict-boolean
//! that string is not `true` and the branch would be skipped. Under
//! truthy semantics it fires. These tests pin the truthy contract.

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
    let tmp = std::env::temp_dir().join(format!("ruuter-switch-truthy-{}", uniq()));
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

/// Reporter's exact case: `${a && b}` with both operands truthy
/// non-empty strings evaluates to the second string. Under strict
/// boolean this misses; under truthy it fires.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_when_and_expression_returns_non_boolean_truthy() {
    let dsl = r#"
setup:
  assign:
    requestId: "abc-123"
    poll: "yes"
  next: check_poll

check_poll:
  switch:
    - condition: "${requestId && poll}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "trigger", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "trigger",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched");
}

/// Same expression, one operand empty → falsy → falls through to
/// default. Confirms that "truthy" doesn't degrade to "always match."
#[tokio::test(flavor = "current_thread")]
async fn switch_falls_through_when_and_expression_returns_empty_string() {
    let dsl = r#"
setup:
  assign:
    requestId: "abc-123"
    poll: ""
  next: check_poll

check_poll:
  switch:
    - condition: "${requestId && poll}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "trigger", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "trigger",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "default");
}

/// A non-empty string literal in the condition also matches. Regression
/// guard for the case where a DSL author writes `${someString}`
/// (perhaps a header value) and expects the branch to fire when the
/// string is present.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_on_non_empty_string() {
    let dsl = r#"
setup:
  assign:
    who: "alice"
  next: check_who

check_who:
  switch:
    - condition: "${who}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "who", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "who",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched");
}

/// Numeric truthy: any non-zero number fires; zero does not. JS parity.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_on_nonzero_number_falls_through_on_zero() {
    let dsl_nonzero = r#"
check:
  switch:
    - condition: "${42}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "n1", dsl_nonzero);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "n1",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched");

    let dsl_zero = r#"
check:
  switch:
    - condition: "${0}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "n2", dsl_zero);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "n2",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "default");
}

/// Baseline unchanged: literal `${true}` and `${false}` still work.
#[tokio::test(flavor = "current_thread")]
async fn switch_boolean_baseline_still_works() {
    let dsl = r#"
check:
  switch:
    - condition: "${true}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "b", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "b",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched");
}

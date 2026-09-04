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
//!
//! **Engine parity.** The `is_truthy` helper in `src/steps/switch.rs`
//! operates on `serde_json::Value` after `ScriptEngine::evaluate` — it
//! is engine-agnostic. This test suite passes byte-identically on
//! both scripting backends. CI runs both (`boa` and `quickjs` jobs);
//! locally verify with:
//!
//! ```bash
//! cargo test --test switch_truthy_conditions                          # boa (default)
//! cargo test --no-default-features --features scripting-quickjs \
//!            --test switch_truthy_conditions                          # quickjs
//! ```

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

/// `${a || b}` fires when either operand is truthy — JS short-circuit
/// returns the first truthy value (or the last value if all falsy).
/// Not the reporter's direct case but sister to `&&`; test to lock
/// down parity across both truthy-composing operators.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_on_or_expression_with_first_truthy() {
    let dsl = r#"
setup:
  assign:
    a: "hello"
    b: ""
  next: check

check:
  switch:
    - condition: "${a || b}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "or1", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "or1",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched");
}

/// `${a || b}` where both operands are falsy — no branch fires,
/// falls through to `next: default`.
#[tokio::test(flavor = "current_thread")]
async fn switch_falls_through_when_or_expression_returns_falsy() {
    let dsl = r#"
setup:
  assign:
    a: ""
    b: 0
  next: check

check:
  switch:
    - condition: "${a || b}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "or2", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "or2",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "default");
}

/// Ordering guard: multiple branches, only the first truthy match
/// wins. Regression against a bug where every truthy branch would
/// try to fire (there isn't one, but the guard is cheap and pins
/// the "first match wins" contract in the presence of the new
/// truthy semantics).
#[tokio::test(flavor = "current_thread")]
async fn switch_first_truthy_match_wins_when_multiple_would_fire() {
    let dsl = r#"
setup:
  assign:
    x: "hello"
  next: check

check:
  switch:
    - condition: "${x}"
      next: first
    - condition: "${x.length > 0}"
      next: second
  next: default

first:
  return: { picked: "first" }
  next: end

second:
  return: { picked: "second" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "first", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "first",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "first");
}

/// Object and array literal expressions are truthy — JS parity for
/// the empty-collection case. `${[]}` and `${{}}` both fire.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_on_object_and_array_literals_even_when_empty() {
    let dsl_array = r#"
check:
  switch:
    - condition: "${[]}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "arr", dsl_array);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "arr",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched", "empty array is truthy");

    let dsl_obj = r#"
check:
  switch:
    - condition: "${({})}"
      next: matched
  next: default

matched:
  return: { picked: "matched" }
  next: end

default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "GET", "obj", dsl_obj);
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "obj",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "matched", "empty object is truthy");
}

/// The reporter's setup end-to-end: values come from request headers,
/// assigned into DSL variables, then the switch fires on `${a && b}`.
/// This is closer to how the bug was originally reported.
#[tokio::test(flavor = "current_thread")]
async fn switch_matches_when_headers_bind_to_variables_and_and_expression_fires() {
    let dsl = r#"
setup:
  assign:
    requestId: "${incoming.headers['x-request-id']}"
    poll: "${incoming.headers['x-poll']}"
  next: check_poll

check_poll:
  switch:
    - condition: "${requestId && poll}"
      next: poll_remaining
  next: local_search

poll_remaining:
  return: { picked: "poll_remaining" }
  next: end

local_search:
  return: { picked: "local_search" }
  next: end
"#;
    let router = build_router("svc", "GET", "route", dsl);
    // execute_dsl signature is (project, method, path, body, query, headers, origin).
    let mut headers = HashMap::new();
    headers.insert("x-request-id".to_string(), "abc-123".to_string());
    headers.insert("x-poll".to_string(), "yes".to_string());
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "route",
            HashMap::new(),
            HashMap::new(),
            headers,
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "poll_remaining");
}

/// Same DSL, header missing → falsy → fall through to local_search.
/// Confirms the reporter's opposite-branch case still works.
#[tokio::test(flavor = "current_thread")]
async fn switch_falls_through_when_reporter_header_setup_has_missing_input() {
    let dsl = r#"
setup:
  assign:
    requestId: "${incoming.headers['x-request-id']}"
    poll: "${incoming.headers['x-poll']}"
  next: check_poll

check_poll:
  switch:
    - condition: "${requestId && poll}"
      next: poll_remaining
  next: local_search

poll_remaining:
  return: { picked: "poll_remaining" }
  next: end

local_search:
  return: { picked: "local_search" }
  next: end
"#;
    let router = build_router("svc", "GET", "route2", dsl);
    // Only requestId set; poll is missing → header lookup returns
    // undefined → `${requestId && poll}` short-circuits to
    // undefined → falsy → falls through.
    // Signature: (project, method, path, body, query, headers, origin).
    let mut headers = HashMap::new();
    headers.insert("x-request-id".to_string(), "abc-123".to_string());
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "route2",
            HashMap::new(),
            HashMap::new(),
            headers,
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "local_search");
}

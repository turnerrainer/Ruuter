//! Issue #85 — template step's `headers:` sent literal `"null"` for
//! null-evaluating expressions.
//!
//! Reporter (angryziber): a template step with
//! `headers: { another-header: "${incoming.headers['no-such-header']}" }`
//! passed the child DSL `incoming.headers.another-header = "null"`
//! (the four-byte string), instead of omitting the header. Pre-fix,
//! this could then propagate to a downstream http step as a real
//! `X-Foo: null` header on the wire — the same class of bug issue
//! #57 fixed at the http_client / return_step seams but never
//! extended to the template step.
//!
//! Fix filters `Value::Null` out of the template's child_headers map
//! at `src/steps/template.rs`, matching the behaviour every other
//! outbound header seam has always had.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
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
    let tmp = std::env::temp_dir().join(format!("ruuter-85-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

/// The reporter's minimal case. A template step passes a header
/// whose value evaluates to `undefined`; the child DSL echoes what
/// it received under `incoming.headers`. Pre-fix, the echoed header
/// was the string `"null"`. Post-fix, the key is absent entirely.
#[tokio::test]
async fn null_valued_template_header_is_omitted_not_stringified() {
    let router = build(&[
        (
            "svc/GET/templates/echo.yml",
            r#"
respond:
  return:
    got_it: "${incoming.headers['x-forwarded']}"
    key_present: "${incoming.headers['x-forwarded'] !== undefined}"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/call.yml",
            r#"
fetch:
  template: templates/echo
  request_type: GET
  headers:
    x-forwarded: "${incoming.headers['no-such-header']}"
  result: r
  next: shape

shape:
  return: { child: "${r}" }
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "call",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    let body = serde_json::to_string(&r.value).unwrap_or_default();
    // Absence marker set by the child DSL — the header must not be
    // present on the child's `incoming.headers` map.
    assert!(
        body.contains("\"key_present\":false"),
        "template must omit null-valued header from child; got: {body}"
    );
    // Belt-and-braces: no literal "null" string leaked through as
    // the value.
    assert!(
        !body.contains("\"got_it\":\"null\""),
        "template must NOT stringify null to the literal \"null\"; got: {body}"
    );
}

/// Non-null header values still forward correctly. Guards against a
/// regression where the filter accidentally drops all values.
#[tokio::test]
async fn non_null_template_headers_still_forward() {
    let router = build(&[
        (
            "svc/GET/templates/echo.yml",
            r#"
respond:
  return:
    got_it: "${incoming.headers['x-forwarded']}"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/call.yml",
            r#"
fetch:
  template: templates/echo
  request_type: GET
  headers:
    x-forwarded: "hello"
  result: r
  next: shape

shape:
  return: { child: "${r}" }
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "call",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    let body = serde_json::to_string(&r.value).unwrap_or_default();
    assert!(
        body.contains("hello"),
        "declared value must reach the child DSL; got: {body}"
    );
}

/// Numbers / booleans still stringify (existing contract for
/// non-string, non-null values — `X-Count: 5` etc.).
#[tokio::test]
async fn non_string_non_null_template_headers_stringify() {
    let router = build(&[
        (
            "svc/GET/templates/echo.yml",
            r#"
respond:
  return:
    n: "${incoming.headers['x-count']}"
    b: "${incoming.headers['x-flag']}"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/call.yml",
            r#"
setup:
  assign: { n: 5, b: true }
  next: fetch

fetch:
  template: templates/echo
  request_type: GET
  headers:
    x-count: "${n}"
    x-flag: "${b}"
  result: r
  next: shape

shape:
  return: { child: "${r}" }
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "call",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    let body = serde_json::to_string(&r.value).unwrap_or_default();
    // Header values reach the child as strings (HTTP wire semantics).
    assert!(body.contains("\"n\":\"5\""), "integer stringifies: {body}");
    assert!(
        body.contains("\"b\":\"true\""),
        "boolean stringifies: {body}"
    );
}

/// Mixed batch — one null-valued header dropped, one string-valued
/// kept. Confirms the filter is per-entry, not all-or-nothing.
#[tokio::test]
async fn null_header_dropped_alongside_other_kept_headers() {
    let router = build(&[
        (
            "svc/GET/templates/echo.yml",
            r#"
respond:
  return:
    kept: "${incoming.headers['x-kept']}"
    dropped_present: "${incoming.headers['x-dropped'] !== undefined}"
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/call.yml",
            r#"
fetch:
  template: templates/echo
  request_type: GET
  headers:
    x-kept: "yes"
    x-dropped: "${incoming.headers['no-such-header']}"
  result: r
  next: shape

shape:
  return: { child: "${r}" }
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "call",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");
    let body = serde_json::to_string(&r.value).unwrap_or_default();
    assert!(
        body.contains("\"kept\":\"yes\""),
        "kept header survives: {body}"
    );
    assert!(
        body.contains("\"dropped_present\":false"),
        "dropped header must be absent from child map: {body}"
    );
}

//! Issue #25 regression tests — HTTP step `headers:` / `query:` and
//! return step `headers:` must accept both a YAML mapping AND a
//! single `${expr}` string that evaluates to an object at runtime.
//!
//! Before the fix, a top-level `headers: "${merged_headers}"`
//! failed at *DSL parse time* with:
//!
//!     invalid type: string "${merged_headers}", expected a map
//!
//! because the field was strictly typed `Option<HashMap<String, Value>>`.
//! The script evaluator never got a chance to run. The fix loosens
//! the type to `Option<Value>` and evaluates both shapes at runtime.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::dsl::parser::DslParser;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn write_dsl(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn build_router(root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ))
}

// ============================================================================
// 1. Parse-time: dynamic map expressions must not fail schema validation
// ============================================================================

#[test]
fn dsl_with_expr_headers_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
compute:
  assign:
    hdrs:
      X-Foo: bar
  next: forward
forward:
  call: http.post
  args:
    url: "http://api.example.com/x"
    headers: "${hdrs}"
    body: "${incoming.body}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse must succeed for headers: '${hdrs}'");
    // Sanity: the step is an HTTP step and its `headers` arg is a
    // string (the expression), not a map.
    let step = dsl.get_step("forward").expect("forward step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["args"]["headers"], serde_json::json!("${hdrs}"));
}

#[test]
fn dsl_with_expr_query_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
forward:
  call: http.get
  args:
    url: "http://api.example.com/x"
    query: "${merged_query}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse must succeed for query: '${expr}'");
    let step = dsl.get_step("forward").expect("forward step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["args"]["query"], serde_json::json!("${merged_query}"));
}

#[test]
fn return_step_with_expr_headers_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
reply:
  return: "ok"
  status: 200
  headers: "${response_hdrs}"
"#,
        )
        .expect("parse must succeed for return.headers: '${expr}'");
    let step = dsl.get_step("reply").expect("reply step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["headers"], serde_json::json!("${response_hdrs}"));
}

#[test]
fn dsl_with_mapping_headers_still_parses() {
    // Backwards compat: the traditional YAML-mapping shape must
    // still parse identically.
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
forward:
  call: http.get
  args:
    url: "http://api.example.com/x"
    headers:
      X-Foo: bar
      X-Bar: "${some_var}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse");
    let step = dsl.get_step("forward").expect("forward");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["args"]["headers"]["X-Foo"], "bar");
    assert_eq!(json["args"]["headers"]["X-Bar"], "${some_var}");
}

// ============================================================================
// 2. Runtime: `${expr}` on the ReturnStep headers actually evaluates
// ============================================================================

#[tokio::test]
async fn return_step_expr_headers_evaluate_to_object() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/thing.yml",
        r#"
compute:
  assign:
    response_hdrs:
      X-Foo: "bar"
      X-Trace: "abc"
  next: reply
reply:
  return: "ok"
  status: 200
  headers: "${response_hdrs}"
"#,
    );
    let router = build_router(tmp.path());
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/thing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    // Both keys from the dynamic map should be on the response.
    let hv_foo = resp.headers().get("x-foo").expect("X-Foo present");
    assert_eq!(hv_foo.to_str().unwrap(), "bar");
    let hv_trace = resp.headers().get("x-trace").expect("X-Trace present");
    assert_eq!(hv_trace.to_str().unwrap(), "abc");
}

#[tokio::test]
async fn return_step_mapping_headers_still_evaluate_per_key() {
    // Backwards compat runtime path.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/thing.yml",
        r#"
compute:
  assign:
    dyn: "42"
  next: reply
reply:
  return: "ok"
  status: 200
  headers:
    X-Static: "hi"
    X-Dyn: "${dyn}"
"#,
    );
    let router = build_router(tmp.path());
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/thing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("x-static").unwrap().to_str().unwrap(),
        "hi"
    );
    assert_eq!(resp.headers().get("x-dyn").unwrap().to_str().unwrap(), "42");
}

#[tokio::test]
async fn return_step_expr_headers_non_object_returns_500_with_diagnostic() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/thing.yml",
        r#"
compute:
  assign:
    hdrs: "not-a-map"
  next: reply
reply:
  return: "ok"
  status: 200
  headers: "${hdrs}"
"#,
    );
    let router = build_router(tmp.path());
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/thing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        500,
        "non-object expression on return.headers must surface as 500"
    );
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("must evaluate to an object")
            && body.contains("string"),
        "expected diagnostic mentioning non-object kind, got: {body}"
    );
}

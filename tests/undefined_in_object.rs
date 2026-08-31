//! Regression contract: a script expression that evaluates to a JS
//! object containing `undefined` values must not panic. `JSON.stringify`
//! semantics apply — undefined properties drop from objects, undefined
//! slots become `null` in arrays.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::scripting::ScriptEngine;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn empty_ctx() -> ExecutionContext {
    ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".into(),
    )
}

// ─── 1. Direct scripting-engine contract ──────────────────────────

#[test]
fn object_with_undefined_property_does_not_panic() {
    // An object literal containing `undefined` must serialize
    // without hitting boa's `todo!("undefined to JSON")`.
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${ ({ present: 'yes', missing: undefined }) }".into()),
            &empty_ctx(),
        )
        .expect("must not panic");
    // JSON.stringify semantics: undefined properties are dropped
    // from objects.
    assert_eq!(out, json!({ "present": "yes" }));
}

#[test]
fn object_assign_merges_and_drops_missing_header() {
    // `Object.assign(base, {'x-request-id': incoming.headers['x-request-id']})`
    // where the header is absent must produce `{...base}` — the
    // undefined value is dropped, matching JS spec.
    let engine = ScriptEngine::new();
    let expr = "${ Object.assign({ 'x-forwarded-for': '10.0.0.1' }, \
                { 'x-request-id': incoming.headers['x-request-id'] }) }";
    let out = engine
        .evaluate(&Value::String(expr.into()), &empty_ctx())
        .expect("must not panic when the merged header is undefined");
    // `x-request-id` is dropped; `x-forwarded-for` survives.
    assert_eq!(out, json!({ "x-forwarded-for": "10.0.0.1" }));
}

#[test]
fn nested_object_with_undefined_property_is_walked() {
    // Undefineds one level deep must also drop, otherwise the panic
    // just moves.
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${ ({ outer: { keep: 1, drop: undefined }, sibling: 'ok' }) }".into()),
            &empty_ctx(),
        )
        .expect("must not panic on nested undefined");
    assert_eq!(out, json!({ "outer": { "keep": 1 }, "sibling": "ok" }));
}

#[test]
fn array_with_undefined_slot_serialises_as_null() {
    // JS spec: `JSON.stringify([1, undefined, 3])` → "[1,null,3]".
    // The engine's array branch handled top-level arrays already,
    // but arrays nested inside an object took the object path.
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(
            &Value::String("${ ({ ids: [1, undefined, 3] }) }".into()),
            &empty_ctx(),
        )
        .expect("must not panic on undefined array slot");
    assert_eq!(out, json!({ "ids": [1, null, 3] }));
}

#[test]
fn top_level_undefined_still_maps_to_null() {
    // An expression that itself evaluates to `undefined` must map to
    // `null` in the outer context.
    let engine = ScriptEngine::new();
    let out = engine
        .evaluate(&Value::String("${ undefined }".into()), &empty_ctx())
        .expect("undefined at top-level must not panic");
    assert_eq!(out, Value::Null);
}

// ─── 2. End-to-end — router exercises the same code path ─────────

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

#[tokio::test]
async fn return_headers_from_object_assign_with_missing_header_does_not_500() {
    // Build a headers map by merging a base with a value pulled from
    // `incoming.headers['x-request-id']` that the caller never sent.
    // The header is dropped, the request completes normally.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/echo.yml",
        r#"
compute:
  assign:
    outbound_hdrs: "${Object.assign({'x-static': 'yes'}, {'x-request-id': incoming.headers['x-request-id']})}"
  next: respond
respond:
  return: "ok"
  status: 200
  headers: "${outbound_hdrs}"
"#,
    );
    let router = build_router(tmp.path());
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/echo")
                // NB: intentionally no x-request-id header.
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "missing-header merge must not crash the request; got body: {}",
        String::from_utf8_lossy(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
    );
}

#[tokio::test]
async fn return_headers_from_object_assign_with_present_header_is_forwarded() {
    // Symmetric check: when the header IS present the merged value
    // propagates to the outgoing response — proves present values
    // aren't dropped along with the missing ones.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/echo.yml",
        r#"
compute:
  assign:
    outbound_hdrs: "${Object.assign({'x-static': 'yes'}, {'x-request-id': incoming.headers['x-request-id']})}"
  next: respond
respond:
  return: "ok"
  status: 200
  headers: "${outbound_hdrs}"
"#,
    );
    let router = build_router(tmp.path());
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/echo")
                .header("x-request-id", "abc-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("x-request-id")
            .expect("x-request-id must be forwarded")
            .to_str()
            .unwrap(),
        "abc-123"
    );
    assert_eq!(
        resp.headers()
            .get("x-static")
            .expect("static header must survive the merge")
            .to_str()
            .unwrap(),
        "yes"
    );
}

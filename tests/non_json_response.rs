//! Issues #23 + #24 regression tests — non-JSON response handling.
//!
//! - **#23**: `http.<verb>` step must expose non-JSON upstream
//!   response bodies as `Value::String`, not silently `null`.
//! - **#24**: Return step must emit raw bytes (no JSON quoting /
//!   escaping) when `wrapper: false` AND the return value is a
//!   string, so DSLs can pass XML / HTML / plaintext through.

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

async fn get(router: Arc<DslRouter>, path: &str) -> (u16, HashMap<String, String>, String) {
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

// ============================================================================
// #24 — wrapper: false + string → raw emission (no JSON quoting)
// ============================================================================

#[tokio::test]
async fn wrapper_false_without_content_type_stays_on_json_path() {
    // Deliberate back-compat gate: raw emission requires the DSL
    // to opt in via a non-JSON Content-Type. Without one, we keep
    // the existing shape (JSON-quoted string, application/json).
    // This preserves DSLs that used `wrapper: false` before this
    // fix landed (they got JSON-quoted output and may rely on it).
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/quoted.yml",
        r#"
respond:
  return: "hello"
  wrapper: false
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, headers, body) = get(router, "/svc/quoted").await;
    assert_eq!(status, 200);
    // JSON-quoted (pre-fix behaviour preserved when no Content-Type).
    assert_eq!(body, "\"hello\"");
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
}

#[tokio::test]
async fn wrapper_false_string_respects_dsl_content_type_header() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/xml.yml",
        r#"
respond:
  return: "<root><item>hello</item></root>"
  headers:
    Content-Type: "text/xml"
  wrapper: false
  status: 201
"#,
    );
    let router = build_router(tmp.path());
    let (status, headers, body) = get(router, "/svc/xml").await;
    assert_eq!(status, 201);
    assert_eq!(body, "<root><item>hello</item></root>");
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/xml"),
        "DSL-supplied Content-Type must override default"
    );
}

#[tokio::test]
async fn wrapper_false_object_still_serialises_as_json() {
    // `wrapper: false` on a JSON object must NOT flip to raw
    // emission — objects can't survive as raw text. Only strings do.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/obj.yml",
        r#"
respond:
  return:
    ok: true
    n: 42
  wrapper: false
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, headers, body) = get(router, "/svc/obj").await;
    assert_eq!(status, 200);
    // No wrapper envelope, but still JSON.
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed, serde_json::json!({"ok": true, "n": 42}));
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json"),
        "objects always emit as application/json even with wrapper: false"
    );
}

#[tokio::test]
async fn wrapper_true_default_still_wraps_and_json_serializes_string() {
    // Baseline: without `wrapper: false`, the response wraps in the
    // envelope AND stays JSON — a string comes out quoted.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/wrapped.yml",
        r#"
respond:
  return: "hello"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, headers, body) = get(router, "/svc/wrapped").await;
    assert_eq!(status, 200);
    // Default wrapper on, so we get the envelope with the string
    // JSON-quoted inside.
    assert_eq!(body, r#"{"response":"hello"}"#);
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
}

#[tokio::test]
async fn wrapper_false_number_still_json() {
    // Non-string, non-object types stay on the JSON path so they
    // round-trip as the correct types.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/num.yml",
        r#"
respond:
  return: 42
  wrapper: false
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (_status, headers, body) = get(router, "/svc/num").await;
    assert_eq!(body, "42"); // JSON number, not a string
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
}

// ============================================================================
// #23 — non-JSON upstream response body reaches DSL as Value::String
// ============================================================================
//
// End-to-end via mockito: an upstream returning text/xml must be
// visible via `${result.response.body}` as the raw text (not `null`).

#[tokio::test]
async fn http_step_preserves_non_json_upstream_body() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/xml-endpoint")
        .with_status(200)
        .with_header("content-type", "text/xml")
        .with_body("<root><item>hello</item></root>")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/proxy.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/xml-endpoint"
  result: upstream
  next: respond
respond:
  return: "${{upstream.response.body}}"
  wrapper: false
  headers:
    Content-Type: "text/xml"
  status: 200
"#,
            server.url()
        ),
    );

    // Build router with SSRF permissive for the mock (loopback host).
    let mut config = AppConfig::default();
    config.config_path = tmp.path().to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    let router = Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ));

    let (status, headers, body) = get(router, "/svc/proxy").await;
    assert_eq!(status, 200);
    // The XML string must arrive verbatim — before the fix this
    // came out as `null` because JSON parse failed silently.
    assert_eq!(body, "<root><item>hello</item></root>");
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/xml")
    );
}

#[tokio::test]
async fn http_step_preserves_plaintext_upstream_body() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/plain")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("just some text here")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/echo.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/plain"
  result: r
  next: reply
reply:
  return: "${{r.response.body}}"
  wrapper: false
  headers:
    Content-Type: "text/plain"
  status: 200
"#,
            server.url()
        ),
    );

    let mut config = AppConfig::default();
    config.config_path = tmp.path().to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    let router = Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ));

    let (status, _headers, body) = get(router, "/svc/echo").await;
    assert_eq!(status, 200);
    assert_eq!(body, "just some text here");
}

#[tokio::test]
async fn http_step_still_parses_json_upstream_body() {
    // Regression guard: JSON responses continue to parse as
    // structured values (the #23 fix must be JSON-first).
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/j")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":42,"name":"alice"}"#)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/lookup.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/j"
  result: r
  next: reply
reply:
  return: "${{r.response.body.name}}"
  status: 200
"#,
            server.url()
        ),
    );

    let mut config = AppConfig::default();
    config.config_path = tmp.path().to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    let router = Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ));

    let (status, _headers, body) = get(router, "/svc/lookup").await;
    assert_eq!(status, 200);
    // With wrapper (default), body is envelope-wrapped and the
    // extracted `name` field is a JSON string.
    assert_eq!(body, r#"{"response":"alice"}"#);
}

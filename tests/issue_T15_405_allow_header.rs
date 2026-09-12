//! h2ck.me v1 T-15 — wrong method → 405 Method Not Allowed with
//! `Allow:` header listing the registered methods (RFC 7231 §7.4.1).
//!
//! Pre-fix, `POST /project/existing-path` when only `GET
//! /project/existing-path` was routed returned 404. RFC-strict
//! clients (test frameworks, some corporate proxies) expected 405
//! + `Allow:` so they could pick the right method automatically.
//!
//! Post-fix, when the DSL resolver rejects the current method but
//! the path resolves for at least one OTHER method, we return
//! `405 Method Not Allowed` with:
//!
//! - `Allow: GET, POST, PUT` (comma-separated registered methods)
//! - Body: `{ "error": "Method Not Allowed", "allow": ["GET", "POST", "PUT"] }`
//!
//! Paths that don't resolve for ANY method continue to return 404.
//!
//! Tests written to try to BREAK the fix:
//! - `POST /svc/onlyget` when `GET /svc/onlyget` exists → 405 +
//!   `Allow: GET`.
//! - Multiple methods registered → `Allow:` names all of them,
//!   sorted alphabetically.
//! - Path exists for NO method → 404 (T-15 doesn't downgrade).
//! - Nested-path resolver still triggers 405: `PATCH
//!   /svc/things/42` when only `GET /svc/things` resolves via
//!   path-param stripping → 405 + `Allow: GET`.
//! - Unknown project → 404 (T-15 only fires when project exists).

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

fn build_router(files: &[(&str, &str)]) -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-T15-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
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

const OK_RESPONSE: &str = r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#;

async fn send(router: DslRouter, method: &str, path: &str) -> (StatusCode, String, String) {
    let app = router.build_axum_router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    let status = resp.status();
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, allow, body)
}

#[tokio::test]
async fn wrong_method_returns_405_with_allow_header() {
    let router = build_router(&[("svc/GET/onlyget.yml", OK_RESPONSE)]);
    let (status, allow, body) = send(router, "POST", "/svc/onlyget").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(allow, "GET");
    assert!(body.contains("Method Not Allowed"));
    assert!(body.contains("\"allow\":[\"GET\"]"));
}

#[tokio::test]
async fn multiple_methods_all_appear_in_allow_header_sorted() {
    let router = build_router(&[
        ("svc/GET/foo.yml", OK_RESPONSE),
        ("svc/POST/foo.yml", OK_RESPONSE),
        ("svc/PUT/foo.yml", OK_RESPONSE),
    ]);
    let (status, allow, _body) = send(router, "DELETE", "/svc/foo").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    // Sorted alphabetically: GET, POST, PUT.
    assert_eq!(allow, "GET, POST, PUT");
}

#[tokio::test]
async fn correct_method_still_returns_200() {
    // T-15 must not break the happy path.
    let router = build_router(&[("svc/GET/foo.yml", OK_RESPONSE)]);
    let (status, _allow, body) = send(router, "GET", "/svc/foo").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\":true"));
}

#[tokio::test]
async fn path_that_exists_for_no_method_returns_404() {
    // T-15 must not downgrade genuine 404s to 405. When the path
    // isn't routed at all, we still return 404.
    let router = build_router(&[("svc/GET/known.yml", OK_RESPONSE)]);
    let (status, allow, _body) = send(router, "GET", "/svc/unknown").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        allow.is_empty(),
        "404 must not carry Allow header; got {allow}"
    );
}

#[tokio::test]
async fn unknown_project_returns_404() {
    let router = build_router(&[("svc/GET/foo.yml", OK_RESPONSE)]);
    let (status, allow, _body) = send(router, "GET", "/no-such-project/foo").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(allow.is_empty());
}

#[tokio::test]
async fn path_param_resolver_still_triggers_405() {
    // `GET /svc/things.yml` serves `/svc/things` AND
    // `/svc/things/42` via path-param stripping. A PATCH request
    // to `/svc/things/42` should 405, not 404 — the path resolves
    // for GET.
    let router = build_router(&[("svc/GET/things.yml", OK_RESPONSE)]);
    let (status, allow, _body) = send(router, "PATCH", "/svc/things/42").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(allow, "GET");
}

//! Audit finding 05 + 12 regression tests: Set-Cookie hardening,
//! wrapper opt-in, and finalResponse status defaults.

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

fn write_dsl(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router(dsl_root: &Path, mut config_mut: impl FnMut(&mut AppConfig)) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    config_mut(&mut config);
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

async fn hit(router: Arc<DslRouter>, path: &str) -> (u16, String, Vec<(String, String)>) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.build_axum_router_from_arc().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned(), headers)
}

// ── finding 05: Set-Cookie hardening ─────────────────────────────

#[tokio::test]
async fn set_cookie_string_passes_through_unchanged() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/plain.yml",
        r#"
respond:
  return: "ok"
  status: 200
  headers:
    Set-Cookie: "session=abc; Path=/custom"
"#,
    );
    let (status, body, headers) = hit(build_router(tmp.path(), |_| {}), "/svc/plain").await;
    assert_eq!(status, 200);
    assert_eq!(body, "\"ok\"");
    let cookie = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .expect("Set-Cookie header present");
    assert!(cookie.1.starts_with("session=abc"), "kept DSL string: {}", cookie.1);
}

#[tokio::test]
async fn set_cookie_object_gets_java_parity_defaults() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/cookie.yml",
        r#"
respond:
  return: "ok"
  status: 200
  headers:
    Set-Cookie:
      session: "abc"
"#,
    );
    let (status, _body, headers) = hit(build_router(tmp.path(), |_| {}), "/svc/cookie").await;
    assert_eq!(status, 200);
    let cookie = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .expect("Set-Cookie present");
    let v = &cookie.1;
    assert!(v.contains("session=abc"), "kept payload: {v}");
    assert!(v.contains("HttpOnly"), "added HttpOnly: {v}");
    assert!(v.contains("Secure"), "added Secure: {v}");
    assert!(v.contains("Path=/"), "added Path=/: {v}");
    assert!(v.contains("Max-Age=28800"), "added Max-Age=28800: {v}");
}

#[tokio::test]
async fn set_cookie_object_respects_dsl_override() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/cookie.yml",
        r#"
respond:
  return: "ok"
  status: 200
  headers:
    Set-Cookie:
      session: "abc"
      Max-Age: 60
      HttpOnly: false
"#,
    );
    let (_, _, headers) = hit(build_router(tmp.path(), |_| {}), "/svc/cookie").await;
    let cookie = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .unwrap();
    let v = &cookie.1;
    assert!(v.contains("Max-Age=60"), "kept DSL Max-Age: {v}");
    assert!(!v.contains("Max-Age=28800"));
    assert!(!v.contains("HttpOnly"), "dropped explicit-false HttpOnly: {v}");
}

// ── finding 12: response wrapper ─────────────────────────────────

#[tokio::test]
async fn wrapper_off_by_default_returns_raw_body() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hi.yml",
        r#"
respond:
  return: "hi"
  status: 200
"#,
    );
    let (status, body, _) = hit(build_router(tmp.path(), |_| {}), "/svc/hi").await;
    assert_eq!(status, 200);
    assert_eq!(body, "\"hi\"", "default is raw body (Rust legacy)");
}

#[tokio::test]
async fn wrapper_true_on_step_wraps_body_java_style() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hi.yml",
        r#"
respond:
  return: "hi"
  status: 200
  wrapper: true
"#,
    );
    let (status, body, _) = hit(build_router(tmp.path(), |_| {}), "/svc/hi").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"hi\"}");
}

#[tokio::test]
async fn config_default_wrapper_wraps_when_step_unset() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hi.yml",
        r#"
respond:
  return: "hi"
  status: 200
"#,
    );
    let (status, body, _) = hit(
        build_router(tmp.path(), |c| c.response.default_wrapper = true),
        "/svc/hi",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"hi\"}");
}

#[tokio::test]
async fn step_wrapper_false_overrides_config_true() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hi.yml",
        r#"
respond:
  return: "hi"
  status: 200
  wrapper: false
"#,
    );
    let (status, body, _) = hit(
        build_router(tmp.path(), |c| c.response.default_wrapper = true),
        "/svc/hi",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, "\"hi\"", "step wrapper: false wins over config");
}

// ── finding 13: finalResponse status codes ───────────────────────

#[tokio::test]
async fn dsl_without_response_status_config_takes_effect_when_no_return_fires() {
    let tmp = TempDir::new().unwrap();
    // DSL runs an assign and then falls off the end with no return.
    write_dsl(
        tmp.path(),
        "svc/GET/void.yml",
        r#"
step:
  assign:
    x: 1
"#,
    );
    let (status, _body, _) = hit(
        build_router(tmp.path(), |c| {
            c.response.dsl_without_response_status = Some(204);
        }),
        "/svc/void",
    )
    .await;
    assert_eq!(status, 204, "config-provided 'no body' status honoured");
}

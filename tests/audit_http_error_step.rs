//! Audit finding 04 regression tests — `HttpStep.error:` routes to
//! the named step on upstream non-allowed status.
//!
//! Uses mockito to stand up a fake upstream that returns 500; a
//! DSL configured with `http_codes_allow_list: [200]` and an
//! `error:` step should route to the error step (with the failed
//! upstream response bound under `${result.response.*}`) rather
//! than propagate.
//!
//! Pre-fix, `HttpStep.error` was parsed but never read, and every
//! non-allowed status raised at the http_client layer with no
//! chance for the DSL to handle it.

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

async fn hit(router: Arc<DslRouter>, path: &str) -> (u16, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.build_axum_router_from_arc().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn http_step_error_field_routes_on_non_allowed_status() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/fail")
        .with_status(500)
        .with_body(r#"{"cause":"upstream oops"}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/fail", server.url());
    write_dsl(
        tmp.path(),
        "svc/GET/wrapped.yml",
        &format!(
            r#"
try_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  error: handle_failure
  next: unreachable
handle_failure:
  return: {{ handled: true, upstream_status: "${{r.response.status}}" }}
  status: 503
unreachable:
  return: "should not reach"
  status: 200
"#,
            url = url,
        ),
    );

    let router = build_router(tmp.path(), |c| {
        c.http_codes_allow_list = vec![200];
    });
    let (status, body) = hit(router, "/svc/wrapped").await;
    assert_eq!(status, 503);
    assert_eq!(body, "{\"handled\":true,\"upstream_status\":500}");
    m.assert_async().await;
}

/// Without `error:`, non-allowed status still errors out (Java
/// parity, upstream branch of the `if (getOnErrorStep() != null)`).
#[tokio::test]
async fn http_step_without_error_field_propagates_on_non_allowed_status() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/fail")
        .with_status(500)
        .with_body("nope")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/fail", server.url());
    write_dsl(
        tmp.path(),
        "svc/GET/naked.yml",
        &format!(
            r#"
try_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  next: end
"#,
            url = url,
        ),
    );

    let router = build_router(tmp.path(), |c| {
        c.http_codes_allow_list = vec![200];
    });
    let (status, _body) = hit(router, "/svc/naked").await;
    assert_eq!(status, 500);
    m.assert_async().await;
}

/// Allowed status runs `next:`, ignores `error:`.
#[tokio::test]
async fn http_step_error_field_ignored_on_allowed_status() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/ok")
        .with_status(200)
        .with_body("\"hi\"")
        .with_header("content-type", "application/json")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/ok", server.url());
    write_dsl(
        tmp.path(),
        "svc/GET/happy.yml",
        &format!(
            r#"
try_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  error: handle_failure
  next: happy_path
handle_failure:
  return: "unexpected error branch"
  status: 500
happy_path:
  return: "${{r.response.body}}"
  status: 200
"#,
            url = url,
        ),
    );

    let router = build_router(tmp.path(), |c| {
        c.http_codes_allow_list = vec![200];
    });
    let (status, body) = hit(router, "/svc/happy").await;
    assert_eq!(status, 200);
    assert_eq!(body, "\"hi\"");
    m.assert_async().await;
}

//! Audit finding 13 regression test — default_dsl_in_case_of_exception
//! is invoked on upstream HTTP failure when the step has no local
//! `error:` handler. The fallback DSL sees `${incoming.body.statusCode}`,
//! `${incoming.body.responseBody}`, and `${incoming.body.failedRequestId}`.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::{AppConfig, DefaultHttpDslConfig};
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

fn build_router(dsl_root: &Path, mut m: impl FnMut(&mut AppConfig)) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    m(&mut config);
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let mut engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    if let Some(cfg) = config.default_dsl_in_case_of_exception.clone() {
        engine = engine.with_default_exception_dsl(cfg);
    }
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

#[tokio::test]
async fn default_exception_dsl_fires_on_non_allowed_status() {
    let mut server = mockito::Server::new_async().await;
    let m_upstream = server
        .mock("GET", "/fail")
        .with_status(500)
        .with_body(r#"{"cause":"oops"}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/fail", server.url());
    // The fallback DSL uses state.set to record it fired — the
    // primary DSL still errors (Java behaviour), but we can assert
    // on side effects of the fallback via a follow-up state.get.
    write_dsl(
        tmp.path(),
        "framework/POST/on-failure.yml",
        r#"
record:
  state:
    set:
      key: last_failure_status
      value: "${incoming.body.statusCode}"
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "framework/GET/inspect.yml",
        r#"
peek:
  state:
    get: { key: last_failure_status, into: seen }
  next: reply
reply:
  return:
    seen: "${seen}"
  status: 200
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/GET/broken.yml",
        &format!(
            r#"
call:
  call: http.get
  args:
    url: "{url}"
  result: r
"#,
            url = url
        ),
    );

    let router = build_router(tmp.path(), |c| {
        c.http_codes_allow_list = vec![200];
        c.default_dsl_in_case_of_exception = Some(DefaultHttpDslConfig {
            dsl: "on-failure".into(),
            request_type: "POST".into(),
            project: "framework".into(),
            body: HashMap::new(),
            query: HashMap::new(),
            headers: HashMap::new(),
        });
    });

    // Fire the request that will trigger the fallback.
    let req = Request::builder()
        .method("GET")
        .uri("/svc/broken")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .clone()
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    // Primary DSL still 500s (Java parity).
    assert_eq!(resp.status().as_u16(), 500);

    // Inspect the state the fallback wrote. NOTE: state is
    // project-scoped, so we peek inside `framework`.
    let req2 = Request::builder()
        .method("GET")
        .uri("/framework/inspect")
        .body(Body::empty())
        .unwrap();
    let resp2 = router
        .build_axum_router_from_arc()
        .oneshot(req2)
        .await
        .unwrap();
    assert_eq!(resp2.status().as_u16(), 200);
    let bytes = to_bytes(resp2.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("\"seen\":500"),
        "fallback DSL should have recorded upstream 500 in state: {body}"
    );
    m_upstream.assert_async().await;
}

#[tokio::test]
async fn default_exception_dsl_skipped_when_local_error_handler_set() {
    let mut server = mockito::Server::new_async().await;
    let m_upstream = server
        .mock("GET", "/fail")
        .with_status(500)
        .with_body("nope")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/fail", server.url());
    write_dsl(
        tmp.path(),
        "framework/POST/on-failure.yml",
        r#"
record:
  state:
    set:
      key: last_failure_status
      value: "${incoming.body.statusCode}"
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "framework/GET/inspect.yml",
        r#"
peek:
  state:
    get: { key: last_failure_status, into: seen }
  next: reply
reply:
  return:
    seen: "${(typeof seen === 'undefined' || seen === null) ? 'unset' : seen}"
  status: 200
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/GET/broken.yml",
        &format!(
            r#"
call:
  call: http.get
  args:
    url: "{url}"
  result: r
  error: handle
handle:
  return: "handled locally"
  status: 200
"#,
            url = url
        ),
    );

    let router = build_router(tmp.path(), |c| {
        c.http_codes_allow_list = vec![200];
        c.default_dsl_in_case_of_exception = Some(DefaultHttpDslConfig {
            dsl: "on-failure".into(),
            request_type: "POST".into(),
            project: "framework".into(),
            body: HashMap::new(),
            query: HashMap::new(),
            headers: HashMap::new(),
        });
    });

    // Local error: handler wins → 200, no state write in framework.
    let req = Request::builder()
        .method("GET")
        .uri("/svc/broken")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .clone()
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let req2 = Request::builder()
        .method("GET")
        .uri("/framework/inspect")
        .body(Body::empty())
        .unwrap();
    let resp2 = router
        .build_axum_router_from_arc()
        .oneshot(req2)
        .await
        .unwrap();
    let bytes = to_bytes(resp2.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("\"seen\":\"unset\""),
        "fallback should NOT have fired: {body}"
    );
    m_upstream.assert_async().await;
}

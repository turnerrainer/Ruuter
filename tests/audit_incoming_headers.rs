//! Audit finding 07 regression test — config-level incoming_requests.headers
//! are injected into every request context and visible via
//! ${incoming.headers.*}.

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
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

#[tokio::test]
async fn config_incoming_headers_appear_in_context() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/site.yml",
        r#"
respond:
  return: "${incoming.headers['x-ruuter-site']}"
  status: 200
"#,
    );
    let router = build_router(tmp.path(), |c| {
        c.incoming_requests
            .headers
            .insert("X-Ruuter-Site".into(), "prod-eu-west".into());
    });

    let req = Request::builder()
        .method("GET")
        .uri("/svc/site")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), "\"prod-eu-west\"");
}

/// Java semantics: config header OVERRIDES a client-supplied header
/// of the same name (`putAll`).
#[tokio::test]
async fn config_incoming_headers_override_client_header() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/tag.yml",
        r#"
respond:
  return: "${incoming.headers['x-canary']}"
  status: 200
"#,
    );
    let router = build_router(tmp.path(), |c| {
        c.incoming_requests
            .headers
            .insert("X-Canary".into(), "server-forced".into());
    });

    let req = Request::builder()
        .method("GET")
        .uri("/svc/tag")
        .header("X-Canary", "client-attempted")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), "\"server-forced\"");
}

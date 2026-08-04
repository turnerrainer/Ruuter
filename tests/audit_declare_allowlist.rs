//! Audit finding 10 regression tests — `declare` step's
//! `allowed_body`/`allowed_header`/`allowed_params` and structured
//! `allowlist:` block are enforced at request time. Java-parity:
//! filterFields strips extras, checkFields (POST body / GET query)
//! errors on missing declared fields.

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

fn build_router(dsl_root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
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

async fn post_json(router: Arc<DslRouter>, path: &str, body: serde_json::Value) -> (u16, String) {
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = router.build_axum_router_from_arc().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Flat `allowed_body` filters extras from the POST body — the
/// DSL sees only whitelisted fields.
#[tokio::test]
async fn allowed_body_filters_extras_on_post() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declare:
  call: declare
  allowed_body: [ userId, amount ]
reply:
  return:
    seen: "${JSON.stringify(incoming.body)}"
  status: 200
"#,
    );
    let (status, body) = post_json(
        build_router(tmp.path()),
        "/svc/create",
        serde_json::json!({ "userId": 42, "amount": 100, "attacker": "extra" }),
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.contains("userId"));
    assert!(body.contains("amount"));
    assert!(!body.contains("attacker"), "extra field must be stripped: {body}");
}

/// checkFields fires on POST — missing declared field is a hard 500.
#[tokio::test]
async fn allowed_body_check_fires_on_missing_field_post() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declare:
  call: declare
  allowed_body: [ userId, amount ]
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json(
        build_router(tmp.path()),
        "/svc/create",
        serde_json::json!({ "userId": 42 }),
    )
    .await;
    assert_eq!(status, 500);
    assert!(body.contains("Field missing: amount"), "diagnostic: {body}");
}

/// Structured `allowlist:` form (the Java-Ruuter "extensible" shape
/// with `{ field: <name> }` entries) is honoured just like the flat
/// form. Explicit legacy fields still WIN over structured.
#[tokio::test]
async fn structured_allowlist_produces_same_filter() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declare:
  call: declare
  allowlist:
    body:
      - field: userId
      - field: amount
reply:
  return:
    seen: "${JSON.stringify(incoming.body)}"
  status: 200
"#,
    );
    let (status, body) = post_json(
        build_router(tmp.path()),
        "/svc/create",
        serde_json::json!({ "userId": 42, "amount": 100, "attacker": "extra" }),
    )
    .await;
    assert_eq!(status, 200);
    assert!(!body.contains("attacker"), "extra field must be stripped: {body}");
}

//! DSL hot-reload — atomic-publish behaviour.
//!
//! Tests the swap primitive (`DslRouter::publish_dsls`) end-to-end.
//! We drive the router directly rather than through the file watcher
//! because filesystem-event timing is flaky in CI. A separate,
//! ignored-by-default test exercises the real `notify` watcher.

use axum::body::to_bytes;
use axum::body::Body;
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

/// Build a router+engine sharing the same `ArcSwap`, plus the router
/// handle, mirroring how `main.rs` wires them for hot-reload.
fn build(config: AppConfig, constants: HashMap<String, String>) -> Arc<DslRouter> {
    let loader = DslLoader::new(config.clone(), constants);
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

fn write_ping(dsl_root: &Path, project: &str, body: &str) {
    let dir = dsl_root.join(project).join("GET");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("ping.yml"), body).unwrap();
}

async fn hit(app: axum::Router, project: &str) -> (u16, String) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/{}/ping", project))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Baseline: before publish, hitting `/reload-test/ping` returns "v1".
/// After publish with a rewritten DSL, the same path returns "v2".
/// The router `Arc` is stable across the swap — only the ArcSwap
/// pointer changes.
#[tokio::test]
async fn publish_dsls_makes_new_route_body_visible() {
    let tmp = TempDir::new().unwrap();
    let dsl_root = tmp.path().to_path_buf();
    write_ping(
        &dsl_root,
        "reload-test",
        "response:\n  status: 200\n  return: v1\n",
    );

    let mut config = AppConfig::default();
    config.config_path = dsl_root.clone();
    // Test binds on 127.0.0.1 loopback; disable private-network SSRF
    // blocklist so this harness matches other test harnesses.
    config.internal_requests.block_private_networks = false;

    let router = build(config.clone(), HashMap::new());

    // v1 baseline
    let (status, body) = hit(router.clone().build_axum_router_from_arc(), "reload-test").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"v1\"}");

    // Rewrite the same DSL and re-load through the same public path
    // the watcher uses.
    write_ping(
        &dsl_root,
        "reload-test",
        "response:\n  status: 200\n  return: v2\n",
    );
    let loader = DslLoader::new(config, HashMap::new());
    let loaded = loader.load_everything().unwrap();
    router.publish_dsls(loaded.http, loaded.guards);

    // v2 visible on the SAME router Arc — swap is atomic and internal
    let (status, body) = hit(router.build_axum_router_from_arc(), "reload-test").await;
    assert_eq!(status, 200);
    assert_eq!(body, "{\"response\":\"v2\"}");
}

/// A DSL added after boot (net-new file, not just a rewrite) becomes
/// reachable after publish. Verifies the swap replaces the whole
/// tree, not just existing entries.
#[tokio::test]
async fn publish_dsls_exposes_new_route() {
    let tmp = TempDir::new().unwrap();
    let dsl_root = tmp.path().to_path_buf();
    write_ping(
        &dsl_root,
        "reload-test",
        "response:\n  status: 200\n  return: seed\n",
    );

    let mut config = AppConfig::default();
    config.config_path = dsl_root.clone();
    config.internal_requests.block_private_networks = false;

    let router = build(config.clone(), HashMap::new());

    // Net-new project, not present at boot
    write_ping(
        &dsl_root,
        "new-project",
        "response:\n  status: 201\n  return: fresh\n",
    );
    let loader = DslLoader::new(config, HashMap::new());
    let loaded = loader.load_everything().unwrap();
    router.publish_dsls(loaded.http, loaded.guards);

    let (status, body) = hit(router.build_axum_router_from_arc(), "new-project").await;
    assert_eq!(status, 201);
    assert_eq!(body, "{\"response\":\"fresh\"}");
}

/// A DSL removed after boot returns 404 after publish. Verifies the
/// swap replaces the whole tree (stale entries don't linger).
#[tokio::test]
async fn publish_dsls_removes_deleted_route() {
    let tmp = TempDir::new().unwrap();
    let dsl_root = tmp.path().to_path_buf();
    write_ping(
        &dsl_root,
        "reload-test",
        "response:\n  status: 200\n  return: present\n",
    );

    let mut config = AppConfig::default();
    config.config_path = dsl_root.clone();
    config.internal_requests.block_private_networks = false;

    let router = build(config.clone(), HashMap::new());

    // Confirm baseline reachability
    let (status, _) = hit(router.clone().build_axum_router_from_arc(), "reload-test").await;
    assert_eq!(status, 200);

    // Delete the DSL and republish
    fs::remove_file(dsl_root.join("reload-test/GET/ping.yml")).unwrap();
    let loader = DslLoader::new(config, HashMap::new());
    let loaded = loader.load_everything().unwrap();
    router.publish_dsls(loaded.http, loaded.guards);

    let (status, _) = hit(router.build_axum_router_from_arc(), "reload-test").await;
    assert_eq!(status, 404);
}

/// The OpenAPI cache is rebuilt on publish. Adding a route surfaces
/// in `/_/openapi.json` on the same router Arc.
#[tokio::test]
async fn publish_dsls_rebuilds_openapi_cache() {
    let tmp = TempDir::new().unwrap();
    let dsl_root = tmp.path().to_path_buf();
    write_ping(
        &dsl_root,
        "openapi-test",
        "response:\n  status: 200\n  return: seed\n",
    );

    let mut config = AppConfig::default();
    config.config_path = dsl_root.clone();
    config.internal_requests.block_private_networks = false;

    let router = build(config.clone(), HashMap::new());

    // Publish a net-new route
    write_ping(
        &dsl_root,
        "openapi-test",
        "response:\n  status: 200\n  return: seed\n",
    );
    let dir = dsl_root.join("openapi-test").join("GET");
    fs::write(
        dir.join("newroute.yml"),
        "response:\n  status: 200\n  return: newroute\n",
    )
    .unwrap();
    let loader = DslLoader::new(config, HashMap::new());
    let loaded = loader.load_everything().unwrap();
    router.publish_dsls(loaded.http, loaded.guards);

    // Fetch /_/openapi.json and confirm the new path is in it.
    // h2ck.me M1 — endpoint lives on `admin_router()` post-fix
    // (route enumeration is no longer public-by-default).
    let req = Request::builder()
        .method("GET")
        .uri("/_/openapi.json")
        .body(Body::empty())
        .unwrap();
    let resp = router.admin_router().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("/openapi-test/newroute"),
        "expected new route in openapi spec, got: {}",
        &body[..body.len().min(500)]
    );
}

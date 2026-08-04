//! Audit finding 17 regression tests — `.optional.` / `.optional_`
//! in a script expression's text coerces null → "" (Java's
//! filterEmptyOptional).

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

fn write(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router(dsl_root: &Path) -> Arc<DslRouter> {
    let mut cfg = AppConfig::default();
    cfg.config_path = dsl_root.to_path_buf();
    cfg.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, cfg, state, ws, engine,
    ))
}

async fn run(router: Arc<DslRouter>, method: &str, path: &str) -> serde_json::Value {
    let r = router
        .execute_dsl(
            "svc",
            method,
            path,
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    r.value.unwrap_or(serde_json::Value::Null)
}

/// The `.optional.` marker on an object path coerces the null
/// result to `""`. Pre-fix Rust returned the JSON string "null" in
/// interpolated context or JS null in whole-string context — either
/// way, not Java-parity.
/// The `.optional.` marker (matches Java's exact substring check)
/// coerces a null LEAF field to `""`. The intermediate `optional`
/// key must exist (else JS throws TypeError on the chained access,
/// same as Java's Nashorn eval path).
#[tokio::test]
async fn optional_marker_coerces_null_leaf_to_empty_string() {
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/tag.yml",
        r#"
init:
  assign:
    payload: { optional: {} }
respond:
  return: "tag=${payload.optional.tag}"
  status: 200
"#,
    );
    let out = run(build_router(tmp.path()), "GET", "tag").await;
    assert_eq!(out, serde_json::json!("tag="), "optional-null coerced to empty: got {out}");
}

/// Whole-string `.optional.<name>` returns "" natively (single-
/// expression result, no interpolation stringification).
#[tokio::test]
async fn optional_marker_coerces_whole_string_null_leaf() {
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/tag.yml",
        r#"
init:
  assign:
    payload: { optional: {} }
respond:
  return: "${payload.optional.tag}"
  status: 200
"#,
    );
    let out = run(build_router(tmp.path()), "GET", "tag").await;
    assert_eq!(out, serde_json::json!(""), "whole-string optional-null → empty: got {out}");
}

/// Without `.optional.` in the expression text, null is preserved.
/// This pins the negative case so we don't accidentally coerce all
/// nulls.
#[tokio::test]
async fn expressions_without_optional_marker_keep_null() {
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/tag.yml",
        r#"
init:
  assign:
    payload: { field: null }
respond:
  return: "${payload.field}"
  status: 200
"#,
    );
    let out = run(build_router(tmp.path()), "GET", "tag").await;
    assert_eq!(out, serde_json::Value::Null, "non-optional null preserved: got {out}");
}

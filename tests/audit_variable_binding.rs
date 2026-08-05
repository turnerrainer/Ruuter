//! Audit finding 16 regression test — variable names with
//! non-identifier characters (dashes, dots, etc.) bind cleanly via
//! `globalThis["<name>"]` instead of crashing the eval with a
//! SyntaxError like `var my-name = ...` used to.

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

/// A variable with an identifier-invalid name (`foo-bar`) used to
/// crash Boa with `SyntaxError` at the `var foo-bar = …` binding.
/// Under the fix, the binding uses `globalThis["foo-bar"]` and the
/// DSL author reaches the value via `${this["foo-bar"]}`.
#[tokio::test]
async fn hyphenated_variable_name_does_not_crash_eval() {
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/hop.yml",
        r#"
step_a:
  assign:
    "foo-bar": "world"
step_b:
  return: "hello ${this['foo-bar']}"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "hop",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap(), serde_json::json!("hello world"));
}

/// Baseline: identifier-valid variable names still bind and can be
/// read via bare identifier lookup (`${myVar}`). The globalThis
/// property assignment shows up in the global scope for JS purposes.
#[tokio::test]
async fn identifier_valid_variable_name_still_works_via_bare_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/hop.yml",
        r#"
step_a:
  assign:
    myVar: "world"
step_b:
  return: "hello ${myVar}"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "hop",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.value.unwrap(), serde_json::json!("hello world"));
}

//! Audit finding 01 regression test — reloadDsl / reload_dsl / reloadDsls
//! step base field triggers a DSL tree republish (Java parity for the
//! `reloadDsl` field in DslInstance). Gated by
//! `dsl.allow_dsl_reloading`.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::hot_reload::StepReloadHandler;
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

fn write(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build(dsl_root: &Path, allow_reload: bool) -> Arc<DslRouter> {
    let mut cfg = AppConfig::default();
    cfg.config_path = dsl_root.to_path_buf();
    cfg.dsl.allow_dsl_reloading = allow_reload;
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
    let router = Arc::new(DslRouter::from_shared(
        http, guards, cfg.clone(), state, ws, engine.clone(),
    ));
    engine.set_reload_handler(Arc::new(StepReloadHandler::new(
        cfg.clone(),
        HashMap::new(),
        router.clone(),
    )));
    router
}

/// Baseline: `/svc/ping` returns `"v1"`. Then rewrite the file, hit
/// a DSL whose first step is `reload_dsl: true`, then hit `/svc/ping`
/// again — should now return `"v2"` because the reload republished
/// the new tree.
#[tokio::test]
async fn reload_dsl_step_republishes_tree() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/ping.yml",
        r#"
respond:
  return: "v1"
  status: 200
"#,
    );
    // Small hop-step that just triggers a reload. reload_dsl on a
    // no-op step is fine — the engine applies it AFTER the step's
    // action.
    write(
        tmp.path(),
        "svc/GET/kick.yml",
        r#"
kick:
  reload_dsl: true
  next: end
"#,
    );

    let router = build(tmp.path(), /* allow_reload */ true);

    // v1 baseline
    let r1 = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r1.value.unwrap(), serde_json::json!("v1"));

    // Rewrite the file on disk to v2 and fire the reload step.
    write(
        tmp.path(),
        "svc/GET/ping.yml",
        r#"
respond:
  return: "v2"
  status: 200
"#,
    );
    router
        .execute_dsl(
            "svc",
            "GET",
            "kick",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();

    // v2 visible on the same router handle — atomic swap.
    let r2 = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r2.value.unwrap(), serde_json::json!("v2"));
}

/// With `allow_dsl_reloading: false`, the reload step logs an error
/// but does NOT republish. Verify by editing the file and firing
/// the step — old body still serves.
#[tokio::test]
async fn reload_dsl_step_no_ops_when_gate_off() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/ping.yml",
        r#"
respond:
  return: "v1"
  status: 200
"#,
    );
    write(
        tmp.path(),
        "svc/GET/kick.yml",
        r#"
kick:
  reload_dsl: true
  next: end
"#,
    );
    let router = build(tmp.path(), /* allow_reload */ false);

    // Edit on disk to v2 and try to reload — should log ERROR, not
    // republish.
    write(
        tmp.path(),
        "svc/GET/ping.yml",
        r#"
respond:
  return: "v2"
  status: 200
"#,
    );
    router
        .execute_dsl(
            "svc",
            "GET",
            "kick",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();

    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.value.unwrap(), serde_json::json!("v1"), "reload was gated off, tree unchanged");
}

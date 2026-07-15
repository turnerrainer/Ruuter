//! Integration tests for the `state` step (#003).
//!
//! These tests build a tiny in-memory project tree (sidestepping the
//! filesystem loader) and exercise `DslRouter::execute_dsl` directly,
//! so we verify routing → state-step → ScriptEngine end-to-end with
//! no service-specific logic in scope.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;

fn build_router_with_dsl(project: &str, method: &str, path: &str, body: &str) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-state-test-{}", uuid()));
    let dir = tmp.join(project).join(method);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{}.yml", path)), body).unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp.clone();

    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let dsls = loader.load_all().expect("load dsls");
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws_registry.clone());
    DslRouter::new(dsls, std::collections::HashMap::new(), cfg, StateStore::new(), ws_registry, engine)
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

#[tokio::test]
async fn counter_survives_across_requests() {
    let dsl = r#"
read:
  state:
    get: { key: "counter", into: current }
  next: bump

bump:
  assign:
    next_value: "${(current == null ? 0 : current) + 1}"
  next: write

write:
  state:
    set: { key: "counter", value: "${next_value}" }
  next: respond

respond:
  return:
    counter: "${next_value}"
  next: end
"#;

    let router = build_router_with_dsl("svc", "POST", "inc", dsl);

    for expected in 1..=5 {
        let res = router.execute_dsl(
            "svc", "POST", "inc",
            HashMap::new(), HashMap::new(), HashMap::new(),
            "test".into(),
        ).await.expect("exec");
        assert_eq!(res.value.unwrap()["counter"], serde_json::json!(expected));
    }
}

#[tokio::test]
async fn projects_are_isolated() {
    // Two projects, each with their own DSL that writes & reads "x".
    let dsl_a = r#"
write:
  state:
    set: { key: "x", value: "alpha" }
  next: read

read:
  state:
    get: { key: "x", into: current }
  next: respond

respond:
  return: { value: "${current}" }
  next: end
"#;
    let dsl_b = r#"
read:
  state:
    get: { key: "x", into: current }
  next: respond

respond:
  return: { value: "${current}" }
  next: end
"#;

    let tmp = std::env::temp_dir().join(format!("ruuter-iso-test-{}", uuid()));
    std::fs::create_dir_all(tmp.join("alpha").join("POST")).unwrap();
    std::fs::create_dir_all(tmp.join("beta").join("POST")).unwrap();
    std::fs::write(tmp.join("alpha/POST/write.yml"), dsl_a).unwrap();
    std::fs::write(tmp.join("beta/POST/read.yml"), dsl_b).unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp.clone();
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let dsls = loader.load_all().expect("load");
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws_registry.clone());
    let router = DslRouter::new(dsls, std::collections::HashMap::new(), cfg, StateStore::new(), ws_registry, engine);

    // Project A writes "x" = "alpha".
    let a = router.execute_dsl(
        "alpha", "POST", "write",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.expect("alpha exec");
    assert_eq!(a.value.unwrap()["value"], serde_json::json!("alpha"));

    // Project B reads "x" — must not see project A's write.
    let b = router.execute_dsl(
        "beta", "POST", "read",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.expect("beta exec");
    assert_eq!(b.value.unwrap()["value"], serde_json::json!(null));
}

#[tokio::test]
async fn concurrent_increments_do_not_panic() {
    // Lost-update IS expected here (read-modify-write isn't atomic at
    // the DSL level — that's #007 territory). What we DO test: no
    // panics, no deadlocks, store remains consistent.
    let dsl = r#"
read:
  state:
    get: { key: "counter", into: current }
  next: bump

bump:
  assign:
    next_value: "${(current == null ? 0 : current) + 1}"
  next: write

write:
  state:
    set: { key: "counter", value: "${next_value}" }
  next: respond

respond:
  return:
    counter: "${next_value}"
  next: end
"#;

    let router = Arc::new(build_router_with_dsl("conc", "POST", "inc", dsl));

    let mut tasks = vec![];
    for _ in 0..50 {
        let r = router.clone();
        tasks.push(tokio::spawn(async move {
            r.execute_dsl(
                "conc", "POST", "inc",
                HashMap::new(), HashMap::new(), HashMap::new(),
                "test".into(),
            ).await.expect("exec")
        }));
    }
    for t in tasks {
        let _ = t.await.expect("join");
    }

    // Final value is in [1, 50] — lost updates allowed, but state must
    // be a real number, not corrupted / missing.
    let res = router.execute_dsl(
        "conc", "POST", "inc",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.unwrap();
    let final_value = res.value.unwrap()["counter"].as_i64().expect("number");
    assert!(final_value >= 1 && final_value <= 51, "got {}", final_value);
}

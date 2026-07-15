//! Task 018 — Java-parity path parameters. One DSL file at
//! `GET/things.yml` serves `/things`, `/things/{id}`, `/things/{id}/legs`.
//! Stripped segments arrive as `incoming.params.pathParams` (array,
//! URL order).

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

fn build(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-pp-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws.clone());
    DslRouter::new(loaded.http, loaded.guards, cfg, StateStore::new(), ws, engine)
}

#[tokio::test]
async fn exact_match_binds_empty_path_params() {
    let router = build(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return: { count: "${incoming.params.pathParams.length}" }
  next: end
"#,
    )]);

    let r = router.execute_dsl(
        "svc", "GET", "things",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["count"], 0);
}

#[tokio::test]
async fn one_trailing_segment_becomes_first_path_param() {
    let router = build(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return: { id: "${incoming.params.pathParams[0]}" }
  next: end
"#,
    )]);

    let r = router.execute_dsl(
        "svc", "GET", "things/abc-123",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["id"], "abc-123");
}

#[tokio::test]
async fn multiple_stripped_segments_preserve_url_order() {
    let router = build(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return: { id: "${incoming.params.pathParams[0]}", sub: "${incoming.params.pathParams[1]}" }
  next: end
"#,
    )]);

    let r = router.execute_dsl(
        "svc", "GET", "things/abc-123/legs",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    let v = r.value.unwrap();
    assert_eq!(v["id"], "abc-123");
    assert_eq!(v["sub"], "legs");
}

#[tokio::test]
async fn more_specific_dsl_wins_over_path_params_fallback() {
    // Both `things.yml` (generic) and `things/legs.yml` (specific) exist.
    // A request to `/things/legs` must hit the specific one, not fall
    // back to generic with pathParams=["legs"].
    let router = build(&[
        (
            "svc/GET/things.yml",
            r#"
respond:
  return: { hit: generic, pp: "${incoming.params.pathParams[0]}" }
  next: end
"#,
        ),
        (
            "svc/GET/things/legs.yml",
            r#"
respond:
  return: { hit: specific }
  next: end
"#,
        ),
    ]);

    let r = router.execute_dsl(
        "svc", "GET", "things/legs",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["hit"], "specific");
}

#[tokio::test]
async fn no_match_at_any_depth_returns_file_not_found() {
    let router = build(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return: { ok: true }
  next: end
"#,
    )]);

    let r = router.execute_dsl(
        "svc", "GET", "other/1/2",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await;
    assert!(r.is_err(), "must not match unrelated path");
}

#[tokio::test]
async fn path_params_do_not_leak_across_projects() {
    let router = build(&[
        ("a/GET/things.yml", "respond: { return: { proj: A, pp: \"${incoming.params.pathParams[0]}\" }, next: end }\n"),
        ("b/GET/other.yml", "respond: { return: { proj: B }, next: end }\n"),
    ]);

    let r = router.execute_dsl(
        "a", "GET", "things/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await.unwrap();
    assert_eq!(r.value.unwrap()["pp"], "x");

    // Project B doesn't have `things` — must not fall back to A's.
    let r = router.execute_dsl(
        "b", "GET", "things/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "t".into(),
    ).await;
    assert!(r.is_err());
}

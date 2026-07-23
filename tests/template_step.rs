//! Task 027 — Template step full recursive DSL invocation.
//!
//! A DSL that says `template: "templates/foo"` loads
//! `<project>/GET/templates/foo.yml` (or POST, per `request_type`),
//! runs it through the shared StepEngine with caller-provided
//! body/query/headers overrides, and binds the result under the
//! caller's `result` name.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-tpl-{}", uuid()));
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
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

#[tokio::test]
async fn template_step_runs_target_dsl_and_binds_result() {
    let router = build(&[
        (
            "svc/GET/templates/user-profile.yml",
            r#"
respond:
  return: { name: "${incoming.body.name}", role: "guest" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/call.yml",
            r#"
fetch:
  template: templates/user-profile
  request_type: GET
  body:
    name: "alice"
  result: profile
  next: shape

shape:
  return: { got: "${profile.response.body}" }
  next: end
"#,
        ),
    ]);

    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "call",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();

    assert_eq!(r.status, 200);
    let body = r.value.unwrap();
    assert_eq!(body["got"]["name"], "alice");
    assert_eq!(body["got"]["role"], "guest");
}

#[tokio::test]
async fn template_missing_target_errors_out() {
    let router = build(&[(
        "svc/GET/broken.yml",
        r#"
fetch:
  template: templates/does-not-exist
  request_type: GET
  result: x
  next: end
"#,
    )]);

    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "broken",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await;
    assert!(r.is_err(), "unresolvable template must be an error");
}

#[tokio::test]
async fn template_default_method_is_get() {
    // No `request_type` specified — should default to GET.
    let router = build(&[
        (
            "svc/GET/templates/pong.yml",
            r#"
respond:
  return: { pong: true }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/ping.yml",
            r#"
call:
  template: templates/pong
  result: r
  next: shape
shape:
  return: { echoed: "${r.response.body.pong}" }
  next: end
"#,
        ),
    ]);

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
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["echoed"], true);
}

#[tokio::test]
async fn template_passes_body_overrides_to_callee() {
    let router = build(&[
        (
            "svc/POST/templates/create-entity.yml",
            r#"
respond:
  return: { created: "${incoming.body.title}" }
  status: 201
  next: end
"#,
        ),
        (
            "svc/POST/creator.yml",
            r#"
mk:
  template: templates/create-entity
  request_type: POST
  body:
    title: "hello world"
  result: out
  next: shape
shape:
  return: { name: "${out.response.body.created}" }
  next: end
"#,
        ),
    ]);

    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "creator",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["name"], "hello world");
}

//! Integration tests for the event-trigger dispatcher (#004).

use ruuter_rs::config::AppConfig;
use ruuter_rs::dsl::loader::DslLoader;
use ruuter_rs::http_client::HttpClient;
use ruuter_rs::state::StateStore;
use ruuter_rs::steps::engine::StepEngine;
use ruuter_rs::triggers::TriggerDispatcher;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

fn build_dispatcher_with(triggers: &[(&str, &str, &str, &str)]) -> Arc<TriggerDispatcher> {
    // triggers: list of (project, channel, key, yaml_body)
    let tmp = std::env::temp_dir().join(format!("ruuter-trig-{}", uuid()));
    for (project, channel, key, body) in triggers {
        let dir = tmp.join(project).join("triggers").join(channel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.yml", key)), *body).unwrap();
    }

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp.clone();

    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let state = StateStore::new();
    let engine = StepEngine::new(HttpClient::new(cfg.http_request_timeout));
    Arc::new(TriggerDispatcher::new(loaded.triggers, state, engine))
}

#[tokio::test]
async fn fires_dsl_for_known_channel_and_key() {
    let dsl = r#"
read:
  state:
    get: { key: "last_value", into: prev }
  next: write

write:
  state:
    set: { key: "last_value", value: "${incoming.body.value}" }
  next: respond

respond:
  return: { ok: true, prev: "${prev}", now: "${incoming.body.value}" }
  next: end
"#;

    let d = build_dispatcher_with(&[("svc", "ticks", "AAPL", dsl)]);
    let ok = d.dispatch("svc", "ticks", "AAPL", json!({"value": 42})).await.unwrap();
    assert!(ok, "expected dispatch to find the DSL");

    // State must survive a second dispatch.
    let ok2 = d.dispatch("svc", "ticks", "AAPL", json!({"value": 99})).await.unwrap();
    assert!(ok2);
}

#[tokio::test]
async fn falls_back_to_default_when_key_not_found() {
    let dsl_default = r#"
respond:
  return: { matched: "default" }
  next: end
"#;
    let d = build_dispatcher_with(&[("svc", "ticks", "_default", dsl_default)]);

    let ok = d.dispatch("svc", "ticks", "TSLA", json!({})).await.unwrap();
    assert!(ok, "expected default DSL to handle unknown key");
}

#[tokio::test]
async fn returns_false_for_unknown_channel() {
    let dsl = r#"
respond:
  return: { unused: true }
  next: end
"#;
    let d = build_dispatcher_with(&[("svc", "ticks", "_default", dsl)]);

    let ok = d.dispatch("svc", "fills", "anything", json!({})).await.unwrap();
    assert!(!ok, "unknown channel should not match — returns false");
}

#[tokio::test]
async fn projects_isolate_their_triggers() {
    let dsl_a = r#"
respond: { return: { from: "alpha" }, next: end }
"#;
    let dsl_b = r#"
respond: { return: { from: "beta" }, next: end }
"#;
    let d = build_dispatcher_with(&[
        ("alpha", "ticks", "_default", dsl_a),
        ("beta",  "ticks", "_default", dsl_b),
    ]);

    // No leak: project A's trigger doesn't fire for project B and vice versa.
    let ok_a = d.dispatch("alpha", "ticks", "X", json!({})).await.unwrap();
    let ok_b = d.dispatch("beta",  "ticks", "X", json!({})).await.unwrap();
    let ok_c = d.dispatch("gamma", "ticks", "X", json!({})).await.unwrap();
    assert!(ok_a);
    assert!(ok_b);
    assert!(!ok_c, "unknown project must not match");
}

#[tokio::test]
async fn non_object_payload_is_wrapped_under_value() {
    let dsl = r#"
respond:
  return: { wrapped: "${incoming.body.value}" }
  next: end
"#;
    let d = build_dispatcher_with(&[("svc", "ticks", "_default", dsl)]);

    let ok = d.dispatch("svc", "ticks", "K", json!(42)).await.unwrap();
    assert!(ok);
}

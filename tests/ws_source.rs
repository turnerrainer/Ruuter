//! Integration test for the generic WebSocket source (#005).
//!
//! Spins up a tiny tokio-tungstenite server on a random local port,
//! sends one JSON message, runs a `WsSource` against it, and asserts
//! the trigger DSL fired and mutated state.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::sources::config::{
    DispatchConfig, OnConnectAction, ReconnectPolicy, SourceConfig, WsSourceConfig,
};
use ruuter_on_rust::sources::ws;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::triggers::TriggerDispatcher;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

async fn start_echo_server(messages: Vec<String>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            use futures::SinkExt;
            // Consume any opening payloads the client sends (e.g. auth, subscribe).
            // We don't need to validate them for this test.
            for m in messages {
                ws.send(Message::Text(m)).await.unwrap();
            }
            // Hold the connection open briefly so the client has time to dispatch.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = ws.send(Message::Close(None)).await;
        }
    });

    (port, handle)
}

fn build_dispatcher(triggers: &[(&str, &str, &str, &str)]) -> (Arc<TriggerDispatcher>, StateStore) {
    let tmp = std::env::temp_dir().join(format!("ruuter-ws-{}", uuid()));
    std::fs::create_dir_all(&tmp).unwrap();
    for (project, channel, key, body) in triggers {
        let dir = tmp.join(project).join("triggers").join(channel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.yml", key)), *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let state = StateStore::new();
    let engine = StepEngine::new(HttpClient::new(&cfg));
    let d = Arc::new(TriggerDispatcher::new(
        loaded.triggers, state.clone(), engine,
    ));
    (d, state)
}

#[tokio::test]
async fn ws_source_dispatches_inbound_message_to_trigger() {
    // Trigger DSL: write incoming.body.value into state.
    let dsl = r#"
write:
  state:
    set: { key: "last", value: "${incoming.body.value}" }
  next: respond

respond:
  return: { ok: true }
  next: end
"#;

    let (dispatcher, state) = build_dispatcher(&[("svc", "ticks", "AAPL", dsl)]);

    // The WS server will emit one JSON message that the source must
    // dispatch as (channel=ticks, key=AAPL).
    let msg = json!({"T": "ticks", "S": "AAPL", "value": 42}).to_string();
    let (port, server) = start_echo_server(vec![msg]).await;

    let cfg = WsSourceConfig {
        url: format!("ws://127.0.0.1:{}", port),
        headers: HashMap::new(),
        on_connect: vec![],
        dispatch: DispatchConfig {
            channel: "$.T".to_string(),
            key: "$.S".to_string(),
        },
        reconnect: ReconnectPolicy::default(),
    };

    // Run the source for a short window — it loops forever, so we abort it.
    let source_handle = tokio::spawn({
        let d = dispatcher.clone();
        async move { ws::run("svc".into(), "test".into(), cfg, d, WsRegistry::new()).await }
    });

    // Give the round-trip time to complete.
    tokio::time::sleep(Duration::from_millis(700)).await;
    source_handle.abort();
    let _ = server.await;

    let stored = state.get("svc", "last");
    assert_eq!(stored, Some(json!(42)), "state should reflect the dispatched message");
}

#[tokio::test]
async fn ws_source_handles_array_of_messages_per_frame() {
    let dsl = r#"
write:
  state:
    set: { key: "${incoming.body.S}", value: "${incoming.body.p}" }
  next: end
"#;

    let (dispatcher, state) = build_dispatcher(&[("svc", "trade", "_default", dsl)]);

    // One frame carrying THREE messages — common shape for market-data WS APIs.
    let msg = serde_json::to_string(&json!([
        {"T": "trade", "S": "AAPL", "p": 150},
        {"T": "trade", "S": "MSFT", "p": 380},
        {"T": "trade", "S": "GOOG", "p": 175},
    ])).unwrap();

    let (port, server) = start_echo_server(vec![msg]).await;

    let cfg = WsSourceConfig {
        url: format!("ws://127.0.0.1:{}", port),
        headers: HashMap::new(),
        on_connect: vec![],
        dispatch: DispatchConfig {
            channel: "$.T".into(),
            key:     "$.S".into(),
        },
        reconnect: ReconnectPolicy::default(),
    };

    let h = tokio::spawn({
        let d = dispatcher.clone();
        async move { ws::run("svc".into(), "trade-feed".into(), cfg, d, WsRegistry::new()).await }
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    h.abort();
    let _ = server.await;

    assert_eq!(state.get("svc", "AAPL"), Some(json!(150)));
    assert_eq!(state.get("svc", "MSFT"), Some(json!(380)));
    assert_eq!(state.get("svc", "GOOG"), Some(json!(175)));
}

#[tokio::test]
async fn ws_source_sends_on_connect_payloads() {
    use futures::StreamExt;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<String>>();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut received: Vec<String> = Vec::new();
        // Read up to 2 frames (auth + subscribe), then close.
        for _ in 0..2 {
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                received.push(t);
            }
        }
        let _ = tx.send(received);
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let (dispatcher, _state) = build_dispatcher(&[]);

    let cfg = WsSourceConfig {
        url: format!("ws://127.0.0.1:{}", port),
        headers: HashMap::new(),
        on_connect: vec![
            OnConnectAction::SendJson(json!({"action": "auth", "key": "K"})),
            OnConnectAction::SendJson(json!({"action": "subscribe", "bars": ["AAPL"]})),
        ],
        dispatch: DispatchConfig { channel: "$.T".into(), key: "$.S".into() },
        reconnect: ReconnectPolicy::default(),
    };

    let h = tokio::spawn(async move { ws::run("svc".into(), "feed".into(), cfg, dispatcher, WsRegistry::new()).await });
    let received = tokio::time::timeout(Duration::from_secs(2), rx).await.expect("timeout").unwrap();
    h.abort();
    let _ = server.await;

    assert_eq!(received.len(), 2);
    let parsed: Vec<serde_json::Value> = received.iter().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert_eq!(parsed[0]["action"], "auth");
    assert_eq!(parsed[0]["key"], "K");
    assert_eq!(parsed[1]["action"], "subscribe");
    assert_eq!(parsed[1]["bars"][0], "AAPL");
}

#[test]
fn config_constant_substitution_resolves_known_and_errors_on_missing() {
    use ruuter_on_rust::sources::config::resolve_constants;

    let mut consts: HashMap<String, String> = HashMap::new();
    consts.insert("ws_url".into(), "wss://feed.example.com/v2".into());
    consts.insert("api_key".into(), "k-secret".into());

    let cfg = WsSourceConfig {
        url: "[#ws_url]".into(),
        headers: HashMap::new(),
        on_connect: vec![
            OnConnectAction::SendJson(json!({"key": "[#api_key]"})),
        ],
        dispatch: DispatchConfig {
            channel: "$.T".into(),
            key: "$.S".into(),
        },
        reconnect: ReconnectPolicy::default(),
    };

    let resolved = resolve_constants(&cfg, &consts).unwrap();
    assert_eq!(resolved.url, "wss://feed.example.com/v2");
    match &resolved.on_connect[0] {
        OnConnectAction::SendJson(value) => {
            assert_eq!(value["key"], "k-secret");
        }
    }

    // Missing constant must error rather than send `[#undefined]` literal over the wire.
    let mut bad = cfg.clone();
    bad.url = "[#nope]".into();
    assert!(resolve_constants(&bad, &consts).is_err());
}

#[test]
fn dot_path_extraction_handles_nesting_and_missing() {
    use ruuter_on_rust::sources::config::extract_path;

    let v = json!({"T": "trade", "data": {"sym": "AAPL", "p": 42}});
    assert_eq!(extract_path(&v, "$.T"), Some("trade".into()));
    assert_eq!(extract_path(&v, "$.data.sym"), Some("AAPL".into()));
    assert_eq!(extract_path(&v, "$.data.p"), Some("42".into()));
    assert_eq!(extract_path(&v, "$.missing"), None);
    assert_eq!(extract_path(&v, "$.data.absent"), None);
    // Without the `$.` prefix the expression is invalid → None.
    assert_eq!(extract_path(&v, "T"), None);
}

// Keep an explicit reference to SourceConfig so the type is part of the
// public-API surface this test asserts against.
#[test]
fn source_config_round_trips_yaml() {
    let yaml = r#"
kind: websocket
url: "wss://example.com/v2"
on_connect:
  - send_json: { action: auth, key: K }
dispatch:
  channel: "$.T"
  key: "$.S"
reconnect:
  initial_backoff_ms: 200
  max_backoff_ms: 30000
  jitter: false
"#;
    let parsed: SourceConfig = serde_yaml_ng::from_str(yaml).unwrap();
    match parsed {
        SourceConfig::WebSocket(ws) => {
            assert_eq!(ws.url, "wss://example.com/v2");
            assert_eq!(ws.dispatch.channel, "$.T");
            assert_eq!(ws.reconnect.initial_backoff_ms, 200);
            assert!(!ws.reconnect.jitter);
        }
    }
}

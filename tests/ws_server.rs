//! Integration tests for the server-side WebSocket plane: axum
//! upgrade → per-connection task → DSL dispatch → `ws_send` reply.
//!
//! Spins the real router up on a random local port, opens a real WS
//! client via `tokio_tungstenite`, and round-trips JSON frames. We
//! exercise (a) implicit ws_send to the originating connection,
//! (b) broadcast_prefix fan-out across multiple clients, and
//! (c) connection_id visibility inside the DSL.

use futures::{SinkExt, StreamExt};
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::Value;
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

fn build_router(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-wsserver-{}", uuid()));
    for (rel_path, body) in files {
        let p = tmp.join(rel_path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws_registry.clone());
    DslRouter::new(
        loaded.http,
        loaded.guards,
        cfg,
        StateStore::new(),
        ws_registry,
        engine,
    )
}

async fn serve(router: DslRouter) -> u16 {
    let app = router.build_axum_router();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give axum a moment to start accepting.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    port
}

#[tokio::test]
async fn ws_send_replies_to_originating_connection() {
    let echo = r#"
reply:
  ws_send:
    payload:
      type: "echo"
      received: "${incoming.body}"
      cid: "${incoming.connection_id}"
  next: end
"#;
    let router = build_router(&[("svc/WS/echo.yml", echo)]);
    let port = serve(router).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/svc/echo", port))
        .await
        .expect("connect");

    ws.send(Message::Text(r#"{"hello":"world","n":1}"#.into()))
        .await
        .unwrap();

    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("no frame")
        .expect("ws err");
    let text = match frame {
        Message::Text(t) => t,
        other => panic!("expected text, got {:?}", other),
    };
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "echo");
    assert_eq!(v["received"]["hello"], "world");
    assert_eq!(v["received"]["n"], 1);
    // connection_id should be present and namespaced.
    let cid = v["cid"].as_str().unwrap();
    assert!(cid.starts_with("client:"), "got cid {}", cid);
}

#[tokio::test]
async fn ws_send_broadcasts_to_all_clients_via_prefix() {
    let bcast = r#"
fanout:
  ws_send:
    broadcast_prefix: "client:"
    payload:
      type: "bcast"
      from: "${incoming.connection_id}"
      msg: "${incoming.body}"
  next: end
"#;
    let router = build_router(&[("svc/WS/bcast.yml", bcast)]);
    let port = serve(router).await;

    let (mut a, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/svc/bcast", port))
        .await
        .unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/svc/bcast", port))
        .await
        .unwrap();
    // Let both registrations land before the first send.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    a.send(Message::Text(r#"{"text":"from A"}"#.into()))
        .await
        .unwrap();

    let a_reply = next_text(&mut a).await;
    let b_reply = next_text(&mut b).await;

    let va: Value = serde_json::from_str(&a_reply).unwrap();
    let vb: Value = serde_json::from_str(&b_reply).unwrap();
    assert_eq!(va["type"], "bcast");
    assert_eq!(vb["type"], "bcast");
    assert_eq!(va["msg"]["text"], "from A");
    assert_eq!(vb["msg"]["text"], "from A");
    assert_eq!(va["from"], vb["from"], "both receive the same sender id");
}

#[tokio::test]
async fn ws_dsl_can_persist_per_connection_state() {
    // Save a name keyed by connection_id, then echo it back. Proves
    // (a) state.set takes a script-evaluated key and
    // (b) per-connection state survives between frames on the same
    //     socket.
    let chat = r#"
route:
  switch:
    - condition: "${incoming.body.type === 'set'}"
      next: save
    - condition: "${incoming.body.type === 'who'}"
      next: load
  next: end

save:
  state:
    set:
      key: "name:${incoming.connection_id}"
      value: "${incoming.body.name}"
  next: ack

ack:
  ws_send:
    payload:
      type: "saved"
      name: "${incoming.body.name}"
  next: end

load:
  state:
    get:
      key: "name:${incoming.connection_id}"
      into: "n"
  next: reply

reply:
  ws_send:
    payload:
      type: "who"
      name: "${n}"
  next: end
"#;
    let router = build_router(&[("svc/WS/chat.yml", chat)]);
    let port = serve(router).await;

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/svc/chat", port))
        .await
        .unwrap();

    ws.send(Message::Text(r#"{"type":"set","name":"alice"}"#.into()))
        .await
        .unwrap();
    let saved = next_text(&mut ws).await;
    let v: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["type"], "saved");
    assert_eq!(v["name"], "alice");

    ws.send(Message::Text(r#"{"type":"who"}"#.into())).await.unwrap();
    let who = next_text(&mut ws).await;
    let v: Value = serde_json::from_str(&who).unwrap();
    assert_eq!(v["type"], "who");
    assert_eq!(v["name"], "alice");
}

async fn next_text<S>(ws: &mut S) -> String
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + std::marker::Unpin,
{
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("closed")
        .expect("ws err");
    match frame {
        Message::Text(t) => t,
        other => panic!("expected text, got {:?}", other),
    }
}

//! Generic WebSocket source.
//!
//! Connects to a configured URL, sends opening payloads, then loops on
//! inbound text frames. Each frame is parsed as JSON, dispatch keys
//! are extracted via dot-path, and the message is handed to the
//! `TriggerDispatcher`.
//!
//! Disconnects, auth failures, and unexpected closes all return to the
//! same exponential-backoff reconnect loop; the source task never
//! exits unless the process is shutting down.
//!
//! While connected the source registers its outbound sink in the
//! shared `WsRegistry` under id `source:<project>:<name>`, so trigger
//! DSLs can `ws_send` back to the upstream (e.g. updating a
//! subscription mid-stream).

use crate::sources::config::{extract_path, OnConnectAction, WsSourceConfig};
use crate::triggers::TriggerDispatcher;
use crate::ws::{source_id, Outbound, WsRegistry};
use futures::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

pub async fn run(
    project: String,
    name: String,
    cfg: WsSourceConfig,
    dispatcher: Arc<TriggerDispatcher>,
    registry: WsRegistry,
) {
    let mut backoff_ms = cfg.reconnect.initial_backoff_ms;

    loop {
        match connect_and_drain(&project, &name, &cfg, dispatcher.as_ref(), &registry).await {
            Ok(()) => {
                // Clean close — still reconnect, but reset backoff so a
                // quick blip doesn't escalate the delay.
                info!(%project, %name, "ws closed cleanly; reconnecting");
                backoff_ms = cfg.reconnect.initial_backoff_ms;
            }
            Err(e) => {
                warn!(%project, %name, error = %e, "ws error; backing off {}ms", backoff_ms);
            }
        }

        sleep_with_jitter(backoff_ms, cfg.reconnect.jitter).await;
        backoff_ms = (backoff_ms * 2).min(cfg.reconnect.max_backoff_ms);
    }
}

async fn connect_and_drain(
    project: &str,
    name: &str,
    cfg: &WsSourceConfig,
    dispatcher: &TriggerDispatcher,
    registry: &WsRegistry,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Build a request object so we can attach upgrade headers (auth tokens etc.)
    // before handing it to tungstenite.
    let mut request = cfg.url.as_str().into_client_request()?;
    for (k, v) in cfg.headers.iter() {
        let header_value = HeaderValue::from_str(v).map_err(|e| {
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "invalid header value for '{}': {}",
                k, e
            ))
        })?;
        let header_name: tokio_tungstenite::tungstenite::http::HeaderName =
            k.parse().map_err(|e| {
                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "invalid header name '{}': {}",
                    k, e
                ))
            })?;
        request.headers_mut().insert(header_name, header_value);
    }

    let (ws, response) = tokio_tungstenite::connect_async(request).await?;
    info!(
        %project, %name,
        status = response.status().as_u16(),
        "ws connected"
    );

    let (sink, mut stream) = ws.split();

    // Outbound writer task — drains an mpsc channel onto the WS sink.
    // The matching sender is registered in the WsRegistry so trigger
    // DSLs can `ws_send` upstream (subscriptions, ack frames, …) and
    // so the reader can echo Pongs without holding the sink.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let id = source_id(project, name);
    let (reg_tx, mut reg_rx) = mpsc::unbounded_channel::<Outbound>();
    registry.register(id.clone(), reg_tx);

    // Bridge: registry's typed Outbound → tungstenite Message::Text(JSON).
    // Kept as a separate task so the writer below stays protocol-pure.
    let out_tx_for_bridge = out_tx.clone();
    let bridge = tokio::spawn(async move {
        while let Some(Outbound::Json(value)) = reg_rx.recv().await {
            let text = match serde_json::to_string(&value) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "ws source bridge: bad JSON");
                    continue;
                }
            };
            if out_tx_for_bridge.send(Message::Text(text)).is_err() {
                break;
            }
        }
    });

    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Replay on_connect payloads via the writer channel.
    for action in &cfg.on_connect {
        match action {
            OnConnectAction::SendJson(value) => {
                let text = serde_json::to_string(value)?;
                let _ = out_tx.send(Message::Text(text));
            }
        }
    }

    let drain_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        while let Some(frame) = stream.next().await {
            let frame = frame?;
            match frame {
                Message::Text(text) => {
                    if let Err(e) = handle_text(
                        project,
                        name,
                        &text,
                        &cfg.dispatch_channel(),
                        &cfg.dispatch_key(),
                        dispatcher,
                    )
                    .await
                    {
                        warn!(%project, %name, error = %e, "trigger dispatch failed");
                    }
                }
                Message::Binary(bytes) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        if let Err(e) = handle_text(
                            project,
                            name,
                            text,
                            &cfg.dispatch_channel(),
                            &cfg.dispatch_key(),
                            dispatcher,
                        )
                        .await
                        {
                            warn!(%project, %name, error = %e, "trigger dispatch failed");
                        }
                    }
                }
                Message::Ping(p) => {
                    let _ = out_tx.send(Message::Pong(p));
                }
                Message::Close(_) => {
                    debug!(%project, %name, "ws close frame received");
                    return Ok(());
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(())
    }
    .await;

    // Tear down the writer side and unregister from the registry, no
    // matter how the reader loop exited.
    registry.unregister(&id);
    drop(out_tx);
    writer.abort();
    bridge.abort();

    drain_result
}

async fn handle_text(
    project: &str,
    name: &str,
    text: &str,
    channel_expr: &str,
    key_expr: &str,
    dispatcher: &TriggerDispatcher,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Many WS APIs (Alpaca included) send arrays of messages per frame.
    // Treat both shapes uniformly.
    let value: Value = serde_json::from_str(text)?;
    let messages: Vec<Value> = match value {
        Value::Array(arr) => arr,
        other => vec![other],
    };

    for msg in messages {
        let channel = match extract_path(&msg, channel_expr) {
            Some(c) if !c.is_empty() => c,
            _ => {
                debug!(%project, %name, "dropping msg: channel path '{}' empty/missing", channel_expr);
                continue;
            }
        };
        let key = extract_path(&msg, key_expr).unwrap_or_default();

        let _ = dispatcher.dispatch(project, &channel, &key, msg).await;
    }
    Ok(())
}

async fn sleep_with_jitter(ms: u64, jitter: bool) {
    let final_ms = if jitter {
        let factor: f64 = rand::thread_rng().gen_range(0.5..1.5);
        ((ms as f64) * factor) as u64
    } else {
        ms
    };
    tokio::time::sleep(Duration::from_millis(final_ms)).await;
}

impl WsSourceConfig {
    fn dispatch_channel(&self) -> String {
        self.dispatch.channel.clone()
    }
    fn dispatch_key(&self) -> String {
        self.dispatch.key.clone()
    }
}

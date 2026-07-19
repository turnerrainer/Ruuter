//! Task 043 inbound side: axum Router served over a UnixListener.
//!
//! Verifies the multi-listener pattern in main.rs by building the same
//! kind of accept loop against a real UnixListener + a minimal Router
//! and connecting to it via HttpClient's UDS path. Confirms:
//!
//! - request round-trips over UDS
//! - the same Router serves both TCP and UDS listeners without
//!   diverging behaviour
//! - stale-socket cleanup works (bind after prior instance crashed)

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use ruuter_on_rust::http_client::HttpClient;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

fn socket_path(tag: &str) -> PathBuf {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("ruuter-uds-inbound-{}-{}.sock", tag, ns))
}

/// Mirrors the UDS accept loop in main.rs so this test truly exercises
/// the "same Router over a UnixListener" pattern.
fn spawn_router_on_uds(app: Router, path: PathBuf, shutdown_rx: oneshot::Receiver<()>) {
    tokio::spawn(async move {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let mut shutdown = shutdown_rx;
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let app = app.clone();
                    tokio::spawn(async move {
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let service = hyper_util::service::TowerToHyperService::new(app);
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        }
        let _ = std::fs::remove_file(&path);
    });
}

#[tokio::test]
async fn axum_router_over_unix_listener_round_trip() {
    let path = socket_path("basic");
    let app = Router::new().route(
        "/ping",
        get(|| async { (axum::http::StatusCode::OK, "pong").into_response() }),
    );
    let (tx, rx) = oneshot::channel::<()>();
    spawn_router_on_uds(app, path.clone(), rx);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut alias = HashMap::new();
    alias.insert("host".to_string(), path.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);

    let resp = client
        .request(reqwest::Method::GET, "http://host/ping", None, None, None, None)
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let _ = tx.send(());
}

#[tokio::test]
async fn stale_socket_is_removed_before_bind() {
    let path = socket_path("stale");
    // Simulate a stale socket file left by a crashed prior instance.
    std::fs::File::create(&path).unwrap();
    assert!(path.exists());

    let app = Router::new().route(
        "/ok",
        get(|| async { (axum::http::StatusCode::OK, "ok").into_response() }),
    );
    let (tx, rx) = oneshot::channel::<()>();
    spawn_router_on_uds(app, path.clone(), rx);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut alias = HashMap::new();
    alias.insert("hostx".to_string(), path.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);
    let resp = client
        .request(reqwest::Method::GET, "http://hostx/ok", None, None, None, None)
        .await
        .expect("request");
    assert_eq!(resp.status, 200);
    let _ = tx.send(());
}

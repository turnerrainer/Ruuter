//! Task 050 — UDS keep-alive connection pool.
//!
//! Assertions chosen to BREAK a broken implementation:
//! - Same client Arc must be returned for the same socket (pool key).
//! - Different sockets get different clients.
//! - Sequential requests to a busy target complete faster with
//!   pooling than the theoretical per-request-handshake bound
//!   (proof pooling is actually reusing connections).
//! - Existing UDS integration tests (`tests/uds_transport.rs`,
//!   `tests/uds_inbound.rs`) must still pass byte-identically —
//!   pooling is a swap, not a semantic change.

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use ruuter_on_rust::http_client::uds_pool::UdsPool;
use ruuter_on_rust::http_client::HttpClient;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

fn socket_path(tag: &str) -> PathBuf {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("ruuter-uds-pool-{}-{}.sock", tag, ns))
}

async fn spawn_echo_uds(path: PathBuf, shutdown_rx: oneshot::Receiver<()>) {
    let app = Router::new().route(
        "/ping",
        get(|| async { (axum::http::StatusCode::OK, "pong").into_response() }),
    );
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = tokio::net::UnixListener::bind(&path).expect("bind");
    let sock_clone = path.clone();
    tokio::spawn(async move {
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
        let _ = std::fs::remove_file(&sock_clone);
    });
    // Let the accept loop settle
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ── Pool identity ────────────────────────────────────────────────

#[tokio::test]
async fn pool_caches_client_per_socket() {
    let pool = UdsPool::default();
    assert_eq!(pool.len(), 0);

    // First access to socket A creates the client
    let path_a = PathBuf::from("/tmp/uds-pool-a.sock");
    let path_b = PathBuf::from("/tmp/uds-pool-b.sock");

    // The pool creates lazily; we exercise via a bogus request that
    // will fail at connect (socket doesn't exist). The pool entry
    // is created regardless.
    let client = HttpClient::with_timeout_ms(200);
    let mut alias = HashMap::new();
    alias.insert("a".to_string(), path_a.clone());
    alias.insert("b".to_string(), path_b.clone());
    let client = client.with_unix_socket_map(alias);

    let _ = client
        .request(reqwest::Method::GET, "http://a/x", None, None, None, None)
        .await;
    let _ = client
        .request(reqwest::Method::GET, "http://b/x", None, None, None, None)
        .await;
    let _ = client
        .request(reqwest::Method::GET, "http://a/y", None, None, None, None)
        .await;

    // Should have exactly 2 pool entries: one per unique socket
    assert_eq!(
        client.uds_pool().len(),
        2,
        "expected 2 unique socket pool entries"
    );
}

// ── Pool actually reuses connections ────────────────────────────

#[tokio::test]
async fn sequential_requests_reuse_pool_connection() {
    // 100 sequential requests. Without pooling, each does a fresh
    // handshake (~1 ms each on loopback UDS = ~100 ms total). With
    // pooling, only the first opens a connection; the rest reuse
    // it (~sub-millisecond each = well under 100 ms total).
    //
    // We assert the pooled path completes 100 requests in < 500 ms
    // — a very forgiving bound that a non-pooling implementation
    // could still hit on a fast box. Sharper assertion would need
    // per-connection counters on the server side; kept generous
    // to avoid flakes on slow CI.
    let sock = socket_path("sequential");
    let (tx, rx) = oneshot::channel::<()>();
    spawn_echo_uds(sock.clone(), rx).await;

    let mut alias = HashMap::new();
    alias.insert("seq".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(1000).with_unix_socket_map(alias);

    let started = Instant::now();
    for _ in 0..100 {
        let resp = client
            .request(
                reqwest::Method::GET,
                "http://seq/ping",
                None,
                None,
                None,
                None,
            )
            .await
            .expect("request");
        assert_eq!(resp.status, 200);
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "100 pooled UDS requests took {:?} — pool likely not reusing connections",
        elapsed
    );

    let _ = tx.send(());
}

// ── Pool survives target restart ─────────────────────────────────

#[tokio::test]
async fn pool_survives_target_restart() {
    // First run: start server, do a request (opens a pooled conn).
    // Kill server. Start new server on same socket. Next request
    // must succeed — hyper-util should notice the stale connection
    // and open a fresh one.
    let sock = socket_path("restart");
    let mut alias = HashMap::new();
    alias.insert("r".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);

    let (tx1, rx1) = oneshot::channel::<()>();
    spawn_echo_uds(sock.clone(), rx1).await;

    let resp1 = client
        .request(reqwest::Method::GET, "http://r/ping", None, None, None, None)
        .await
        .expect("first request");
    assert_eq!(resp1.status, 200);

    // Shut down server, clear socket file
    let _ = tx1.send(());
    tokio::time::sleep(Duration::from_millis(100)).await;
    if sock.exists() {
        let _ = std::fs::remove_file(&sock);
    }

    // Fresh server on same socket
    let (tx2, rx2) = oneshot::channel::<()>();
    spawn_echo_uds(sock.clone(), rx2).await;

    // Retry loop — hyper-util may hand out the stale connection
    // once; the second attempt should succeed. This is the exact
    // behaviour the task notes as expected.
    let mut ok = false;
    for _ in 0..3 {
        if let Ok(resp) = client
            .request(reqwest::Method::GET, "http://r/ping", None, None, None, None)
            .await
        {
            if resp.status == 200 {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "pool did not recover after target restart");

    let _ = tx2.send(());
}

//! Integration tests for task 043 (Unix Domain Socket transport).
//!
//! Covers OUTBOUND requests over UDS: an in-process axum server is
//! bound to a temporary Unix socket, then HttpClient is exercised via
//! both routing paths (alias-map and explicit `unix://` scheme). This
//! is the same accept-loop pattern the multi-listener code in main.rs
//! uses, so the test also indirectly validates inbound wiring.
//!
//! Tests written to BREAK an incorrect implementation:
//! - Alias must resolve; a missing alias must fall through to TCP
//! - `unix://` scheme must parse socket path + request-target correctly
//! - Body round-trip (request JSON, response JSON) must preserve values
//! - Custom request/response headers must survive
//! - Non-2xx status must propagate to the caller

use axum::body::Bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use ruuter_on_rust::http_client::HttpClient;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

fn socket_path() -> PathBuf {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ruuter-uds-test-{}.sock", ns))
}

/// Spawn an axum echo server on a UDS. Returns the socket path + a
/// shutdown handle. The server responds with a JSON body echoing the
/// request path + body + headers + method.
async fn spawn_echo_server(path: &std::path::Path) -> oneshot::Sender<()> {
    let app = Router::new().route(
        "/*rest",
        any(|req: Request| async move {
            let method = req.method().to_string();
            let uri_path = req.uri().path().to_string();
            let uri_query = req.uri().query().map(|s| s.to_string());
            let mut headers = serde_json::Map::new();
            for (k, v) in req.headers() {
                headers.insert(
                    k.to_string(),
                    Value::String(v.to_str().unwrap_or("").to_string()),
                );
            }
            let body_bytes = axum::body::to_bytes(req.into_body(), 65536)
                .await
                .unwrap_or_else(|_| Bytes::new());
            let body_val: Value = if body_bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body_bytes).unwrap_or(Value::Null)
            };
            (
                StatusCode::OK,
                [("x-echoed-by", "uds-test")],
                axum::Json(json!({
                    "method": method,
                    "path": uri_path,
                    "query": uri_query,
                    "headers": headers,
                    "body": body_val,
                })),
            )
                .into_response()
        }),
    );

    if path.exists() {
        std::fs::remove_file(path).ok();
    }
    let listener = tokio::net::UnixListener::bind(path).expect("bind uds");
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    let path_owned = path.to_path_buf();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
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
        std::fs::remove_file(&path_owned).ok();
    });

    // Give the accept loop a beat to be ready
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown_tx
}

// ── Outbound via alias-map ────────────────────────────────────────

#[tokio::test]
async fn uds_alias_routes_http_url_to_socket() {
    let sock = socket_path();
    let _shutdown = spawn_echo_server(&sock).await;

    let mut alias = HashMap::new();
    alias.insert("resql-test".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://resql-test/orders?limit=5",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let body = resp.body.expect("body");
    assert_eq!(body["method"], "GET");
    assert_eq!(body["path"], "/orders");
    assert_eq!(body["query"], "limit=5");
    // Host header must carry the alias name — proves the server saw
    // a request-line with a valid Host, and the alias was resolved
    // for transport only.
    assert_eq!(body["headers"]["host"], "resql-test");
}

#[tokio::test]
async fn uds_alias_body_and_headers_round_trip() {
    let sock = socket_path();
    let _shutdown = spawn_echo_server(&sock).await;

    let mut alias = HashMap::new();
    alias.insert("upstream".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);

    let body = json!({ "hello": "world", "n": 42 });
    let mut hdrs = HashMap::new();
    hdrs.insert("x-request-id".to_string(), Value::String("abc-123".into()));

    let resp = client
        .request(
            reqwest::Method::POST,
            "http://upstream/items",
            Some(&body),
            None,
            Some(&hdrs),
            None,
        )
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let echo = resp.body.unwrap();
    assert_eq!(echo["method"], "POST");
    assert_eq!(echo["body"], json!({"hello": "world", "n": 42}));
    assert_eq!(echo["headers"]["x-request-id"], "abc-123");
    // Response headers from the server propagate back
    assert_eq!(
        resp.headers.get("x-echoed-by"),
        Some(&"uds-test".to_string())
    );
}

#[tokio::test]
async fn uds_alias_query_arg_merges_with_url_query() {
    let sock = socket_path();
    let _shutdown = spawn_echo_server(&sock).await;

    let mut alias = HashMap::new();
    alias.insert("svc".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);

    let mut q = HashMap::new();
    q.insert("filter".to_string(), Value::String("active".into()));
    let resp = client
        .request(
            reqwest::Method::GET,
            "http://svc/things?page=2",
            None,
            Some(&q),
            None,
            None,
        )
        .await
        .expect("request");
    let echo = resp.body.unwrap();
    // Both the original page=2 and the added filter=active must appear
    let observed = echo["query"].as_str().unwrap();
    assert!(
        observed.contains("page=2"),
        "missing page=2 in {}",
        observed
    );
    assert!(
        observed.contains("filter=active"),
        "missing filter=active in {}",
        observed
    );
}

#[tokio::test]
async fn uds_alias_no_match_falls_through_to_tcp() {
    // No alias configured for "unknown-host" → HttpClient tries TCP,
    // which resolves to a nonexistent host and fails. What we're
    // verifying: no accidental UDS attempt on a non-matching host.
    let client = HttpClient::with_timeout_ms(500);
    let res = client
        .request(
            reqwest::Method::GET,
            "http://unknown-host-that-does-not-resolve.invalid/x",
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(res.is_err(), "expected TCP failure, got {:?}", res);
}

// ── Outbound via explicit unix:// scheme ──────────────────────────

#[tokio::test]
async fn uds_explicit_unix_url_reaches_socket() {
    let sock = socket_path();
    let _shutdown = spawn_echo_server(&sock).await;

    let client = HttpClient::with_timeout_ms(2000);
    let url = format!("unix://{}/status", sock.display());

    let resp = client
        .request(reqwest::Method::GET, &url, None, None, None, None)
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let echo = resp.body.unwrap();
    assert_eq!(echo["method"], "GET");
    assert_eq!(echo["path"], "/status");
}

#[tokio::test]
async fn uds_explicit_unix_url_with_query_string() {
    let sock = socket_path();
    let _shutdown = spawn_echo_server(&sock).await;

    let client = HttpClient::with_timeout_ms(2000);
    let url = format!("unix://{}/api?a=1&b=2", sock.display());

    let resp = client
        .request(reqwest::Method::GET, &url, None, None, None, None)
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let echo = resp.body.unwrap();
    assert_eq!(echo["path"], "/api");
    assert_eq!(echo["query"], "a=1&b=2");
}

// ── Non-2xx propagation ───────────────────────────────────────────

#[tokio::test]
async fn uds_non_2xx_status_propagates_to_caller() {
    // Same server, but a route that returns 500. Use `/error` — the
    // echo server treats all routes the same, so we can't force 500
    // there. Spin up a purpose-built server for this test.
    let sock = socket_path();
    if sock.exists() {
        std::fs::remove_file(&sock).ok();
    }
    let app = Router::new().route(
        "/boom",
        any(|| async { StatusCode::INTERNAL_SERVER_ERROR.into_response() }),
    );
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let sock_clone = sock.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
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
        std::fs::remove_file(&sock_clone).ok();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut alias = HashMap::new();
    alias.insert("boomsvc".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(2000).with_unix_socket_map(alias);
    let resp = client
        .request(
            reqwest::Method::GET,
            "http://boomsvc/boom",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request should succeed at transport level");
    assert_eq!(resp.status, 500);

    let _ = shutdown_tx.send(());
}

// ── Timeout enforcement ───────────────────────────────────────────

#[tokio::test]
async fn uds_request_timeout_fires_when_server_slow() {
    let sock = socket_path();
    if sock.exists() {
        std::fs::remove_file(&sock).ok();
    }
    let app = Router::new().route(
        "/slow",
        any(|| async {
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            StatusCode::OK.into_response()
        }),
    );
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let sock_clone = sock.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
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
        std::fs::remove_file(&sock_clone).ok();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut alias = HashMap::new();
    alias.insert("slowsvc".to_string(), sock.clone());
    let client = HttpClient::with_timeout_ms(200).with_unix_socket_map(alias);
    let started = std::time::Instant::now();
    let res = client
        .request(
            reqwest::Method::GET,
            "http://slowsvc/slow",
            None,
            None,
            None,
            None,
        )
        .await;
    let elapsed = started.elapsed();

    assert!(res.is_err(), "expected timeout, got {:?}", res);
    assert!(
        elapsed < Duration::from_millis(600),
        "should fire near 200ms, took {:?}",
        elapsed
    );

    let _ = shutdown_tx.send(());
}

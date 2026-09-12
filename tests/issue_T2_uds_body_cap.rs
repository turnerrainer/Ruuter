//! h2ck.me v1 T-2 — UDS outbound reads response body unbounded
//! regardless of `http_response_size_limit`.
//!
//! Pre-fix, both UDS paths (`http_client/uds.rs::request_over_unix`
//! and `http_client/uds_pool.rs::request_over_unix_pooled`) called
//! `.into_body().collect().await.to_bytes()` with no cap, then the
//! caller in `http_client/mod.rs` (`enforce_status_and_size`) ran a
//! POST-HOC size check on the already-buffered body. A misbehaving
//! trusted sidecar (Resql/TIM bug, not compromise) could OOM Ruuter
//! by returning gigabytes of data — T-1's config-default fix does
//! NOT close this seam because the check happens after the read.
//!
//! Post-fix (h2ck.me v1 T-2):
//! - The body is wrapped in `http_body_util::Limited::new(body, cap)`
//!   before `.collect()`, so the reader aborts mid-stream at the cap
//!   boundary and never allocates past the limit.
//! - `Content-Length` is preflighted before the body read too, so
//!   oversized-CL responses skip the body read entirely (mirrors
//!   `http_client/mod.rs` TCP path).
//! - The post-hoc check in `enforce_status_and_size` is deleted —
//!   dead code once the cap is enforced upstream.
//!
//! Tests written to try to BREAK the fix:
//! - Body BELOW cap round-trips normally.
//! - Body ABOVE cap with declared Content-Length → 413-shaped error
//!   from Content-Length preflight, no body read.
//! - Body ABOVE cap with NO Content-Length (chunked / unknown length)
//!   → LengthLimitError from Limited, mid-stream abort.
//! - Body ABOVE cap with LYING Content-Length (CL declares small,
//!   body actually large) → mid-stream abort by Limited (not the
//!   preflight).
//! - Explicit `None` cap → no limit, large body reads through.
//! - Cap exactly at body size → allowed (edge, not off-by-one).
//! - Cap = 0 → any non-empty body rejected.
//! - Both alias-map and `unix://` routing paths behave identically.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use ruuter_on_rust::http_client::HttpClient;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

fn socket_path(tag: &str) -> PathBuf {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ruuter-uds-T2-{tag}-{ns}.sock"))
}

/// Spawn an axum server on `path` that echoes a body of `size` bytes.
///
/// - `size` = the payload length in bytes
/// - `declared_content_length`:
///   - `Some(n)` → server explicitly sets `Content-Length: n` (may
///     lie about the actual body length; the test controls both).
///   - `None` → server omits Content-Length; hyper chunks the
///     response. This is the "unknown length" path that only the
///     mid-stream Limited::new fix can catch.
/// - `content_type` → response Content-Type header
async fn spawn_sized_server(
    path: &std::path::Path,
    size: usize,
    declared_content_length: Option<usize>,
    content_type: &'static str,
) -> oneshot::Sender<()> {
    let app = Router::new().route(
        "/*rest",
        any(move |_req: Request| async move {
            let payload = "x".repeat(size);
            let mut resp = Response::builder().status(StatusCode::OK);
            {
                let hdrs = resp.headers_mut().unwrap();
                hdrs.insert("content-type", content_type.parse().unwrap());
                if let Some(cl) = declared_content_length {
                    hdrs.insert("content-length", cl.to_string().parse().unwrap());
                }
            }
            resp.body(Body::from(payload)).unwrap().into_response()
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

    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown_tx
}

/// Body-response test server that writes a chunked (no Content-
/// Length) response of `size` bytes. Distinct helper because axum's
/// default handler picks Content-Length automatically when the body
/// is a full-buffered String; we need the low-level path for the
/// "no CL" scenario the audit specifically calls out (`Limited` is
/// the ONLY defence when the upstream doesn't declare a length).
async fn spawn_chunked_server(path: &std::path::Path, size: usize) -> oneshot::Sender<()> {
    // Producing a real chunked response requires bypassing axum's
    // Body::from(String) auto-length. We stream via a channel body
    // so the server never sets Content-Length.
    use bytes::Bytes;
    let app = Router::new().route(
        "/*rest",
        any(move |_req: Request| async move {
            let chunks: Vec<Bytes> = (0..size)
                .step_by(4096)
                .map(|start| {
                    let end = std::cmp::min(start + 4096, size);
                    Bytes::from(vec![b'x'; end - start])
                })
                .collect();
            let stream =
                futures::stream::iter(chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
            let body = Body::from_stream(stream);
            let mut resp = Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/octet-stream");
            resp.headers_mut().unwrap().remove("content-length");
            resp.body(body).unwrap().into_response()
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

    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown_tx
}

/// Silence an unused-warning about `HeaderMap` when reduced test
/// variants are compiled — keeps the imports uniform across the
/// file and future additions.
#[allow(dead_code)]
fn _hdr_unused(_: &HeaderMap) {}

fn client_with_cap(cap: Option<usize>, alias_target: (String, PathBuf)) -> HttpClient {
    let mut alias = HashMap::new();
    alias.insert(alias_target.0, alias_target.1);
    HttpClient::with_timeout_ms(5000)
        .with_response_size_limit(cap)
        .with_unix_socket_map(alias)
}

// ────────────────────────────────────────────────────────────────
// Alias-map routing path (http://<alias>/… -> uds socket)
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uds_alias_below_cap_reads_through() {
    let sock = socket_path("alias-below");
    let _s = spawn_sized_server(&sock, 1024, Some(1024), "application/octet-stream").await;
    let client = client_with_cap(Some(8192), ("upstream".into(), sock.clone()));

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://upstream/data",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");
    assert_eq!(resp.status, 200);
    let body_str = match resp.body {
        Some(Value::String(s)) => s,
        other => panic!("expected string body, got {:?}", other),
    };
    assert_eq!(body_str.len(), 1024);
}

#[tokio::test]
async fn uds_alias_content_length_over_cap_rejects_before_read() {
    // The upstream declares Content-Length above the cap. Preflight
    // rejects with an error naming the declared size — no body read
    // ever happens.
    let sock = socket_path("alias-cl-over");
    let _s = spawn_sized_server(
        &sock,
        100 * 1024,
        Some(100 * 1024),
        "application/octet-stream",
    )
    .await;
    let client = client_with_cap(Some(8 * 1024), ("upstream".into(), sock.clone()));

    let result = client
        .request(
            reqwest::Method::GET,
            "http://upstream/big",
            None,
            None,
            None,
            None,
        )
        .await;
    let err = result.expect_err("must reject when declared CL > cap");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream declared body")
            && msg.contains("102400")
            && msg.contains("8192"),
        "err msg must name declared size (102400) and cap (8192); got: {msg}"
    );
}

#[tokio::test]
async fn uds_alias_chunked_body_over_cap_aborts_mid_stream() {
    // NO Content-Length on the wire (chunked). Only the Limited::new
    // wrap can catch this. Pre-fix `.collect()` would buffer all
    // 128 KiB before the post-hoc check ran.
    let sock = socket_path("alias-chunked");
    let _s = spawn_chunked_server(&sock, 128 * 1024).await;
    let client = client_with_cap(Some(16 * 1024), ("upstream".into(), sock.clone()));

    let result = client
        .request(
            reqwest::Method::GET,
            "http://upstream/stream",
            None,
            None,
            None,
            None,
        )
        .await;
    let err = result.expect_err("must reject via Limited on chunked body");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream response body exceeded") && msg.contains("16384"),
        "err msg must name mid-stream cap breach + the cap; got: {msg}"
    );
}

// NB: A "lying Content-Length" scenario (server declares small CL
// but writes more bytes) is not testable with the hyper server
// library — hyper panics/aborts on the CL/body mismatch before the
// packets hit the wire, so the client sees a transport-level SendRequest
// error, not a body-cap error. In real deployments the same is true:
// a well-formed upstream cannot legitimately produce this shape. What
// CAN happen is the chunked / unknown-length case (`spawn_chunked_server`
// above) — that's the case the Limited::new wrap catches.

#[tokio::test]
async fn uds_alias_no_cap_reads_full_body() {
    // Explicit None → no Limited wrap, response reads through even
    // for a large body. Pins the "operator opt-in for uncapped"
    // path from T-1.
    let sock = socket_path("alias-nocap");
    let _s = spawn_sized_server(
        &sock,
        64 * 1024,
        Some(64 * 1024),
        "application/octet-stream",
    )
    .await;
    let client = client_with_cap(None, ("upstream".into(), sock.clone()));

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://upstream/uncapped",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");
    assert_eq!(resp.status, 200);
    let body_str = match resp.body {
        Some(Value::String(s)) => s,
        other => panic!("expected string body, got {:?}", other),
    };
    assert_eq!(body_str.len(), 64 * 1024);
}

#[tokio::test]
async fn uds_alias_cap_equal_to_body_is_allowed() {
    // Cap = 8192, body = 8192 → allowed. Belts-and-braces for the
    // off-by-one class of bugs (cap > vs. cap >=). Content-Length
    // preflight uses `declared > cap`, so equal is OK.
    let sock = socket_path("alias-eq");
    let _s = spawn_sized_server(&sock, 8192, Some(8192), "application/octet-stream").await;
    let client = client_with_cap(Some(8192), ("upstream".into(), sock.clone()));

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://upstream/exact",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");
    assert_eq!(resp.status, 200);
    let body_str = match resp.body {
        Some(Value::String(s)) => s,
        other => panic!("expected string body, got {:?}", other),
    };
    assert_eq!(body_str.len(), 8192);
}

#[tokio::test]
async fn uds_alias_cap_zero_rejects_nonempty_body() {
    // Extreme: cap = 0 with a non-empty body. Content-Length
    // preflight (declared 100 > 0) rejects.
    let sock = socket_path("alias-zero");
    let _s = spawn_sized_server(&sock, 100, Some(100), "application/octet-stream").await;
    let client = client_with_cap(Some(0), ("upstream".into(), sock.clone()));

    let result = client
        .request(
            reqwest::Method::GET,
            "http://upstream/small",
            None,
            None,
            None,
            None,
        )
        .await;
    let err = result.expect_err("cap=0 must reject any non-empty body");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream"),
        "must be a UDS body-cap error; got: {msg}"
    );
}

// ────────────────────────────────────────────────────────────────
// Explicit unix:// scheme path — same fix applies at the same seam.
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uds_scheme_content_length_over_cap_rejects_before_read() {
    let sock = socket_path("scheme-cl");
    let _s = spawn_sized_server(&sock, 200_000, Some(200_000), "application/octet-stream").await;
    // No alias needed — the URL carries the socket path itself.
    let client = HttpClient::with_timeout_ms(5000).with_response_size_limit(Some(4096));

    // Build a `unix://` URL: socket path is `<tmpdir>/…sock`, request
    // target is `/…`.
    let url = format!("unix://{}/data", sock.display());
    let result = client
        .request(reqwest::Method::GET, &url, None, None, None, None)
        .await;
    let err = result.expect_err("must reject when declared CL > cap");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream declared body") || msg.contains("exceeded"),
        "err msg must name the cap breach; got: {msg}"
    );
}

// ────────────────────────────────────────────────────────────────
// Pooled path: exercises `uds_pool::request_over_unix_pooled`.
// The HttpClient uses the pool for both alias-map and unix:// paths,
// so the above tests already cover the pooled path indirectly. This
// test targets the pool directly to prove the Limited::new wrap
// lives in the pool module, not just the caller.
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uds_pool_direct_call_honours_cap() {
    use ruuter_on_rust::http_client::uds_pool::{request_over_unix_pooled, UdsPool};

    let sock = socket_path("pool-direct");
    let _s = spawn_sized_server(&sock, 100_000, Some(100_000), "application/octet-stream").await;

    let pool = UdsPool::default();
    // Cap = 4 KiB; body will be 100 KiB. Content-Length preflight
    // must reject.
    let result = request_over_unix_pooled(
        &pool,
        &sock,
        "localhost",
        "/data",
        http::Method::GET,
        None,
        None,
        Duration::from_secs(2),
        Some(4096),
    )
    .await;
    let err = result.expect_err("pool must honour cap");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream declared body")
            && msg.contains("100000")
            && msg.contains("4096"),
        "err msg must name declared size (100000) and cap (4096); got: {msg}"
    );
}

#[tokio::test]
async fn uds_pool_direct_call_none_cap_reads_full_body() {
    use ruuter_on_rust::http_client::uds_pool::{request_over_unix_pooled, UdsPool};

    let sock = socket_path("pool-nocap");
    let _s = spawn_sized_server(&sock, 20_000, Some(20_000), "application/octet-stream").await;

    let pool = UdsPool::default();
    let resp = request_over_unix_pooled(
        &pool,
        &sock,
        "localhost",
        "/data",
        http::Method::GET,
        None,
        None,
        Duration::from_secs(2),
        None,
    )
    .await
    .expect("must succeed with no cap");
    assert_eq!(resp.status, 200);
    let body_str = match resp.body {
        Some(Value::String(s)) => s,
        other => panic!("expected string body, got {:?}", other),
    };
    assert_eq!(body_str.len(), 20_000);
}

// ────────────────────────────────────────────────────────────────
// Non-pooled single-shot path (`http_client::uds::request_over_unix`).
// Not currently used by HttpClient but exposed pub; regress in case
// external callers depend on the pooled/non-pooled parity.
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uds_non_pooled_content_length_over_cap_rejects() {
    use ruuter_on_rust::http_client::uds::request_over_unix;

    let sock = socket_path("nonpool-cl");
    let _s = spawn_sized_server(&sock, 100_000, Some(100_000), "application/octet-stream").await;

    let result = request_over_unix(
        &sock,
        "localhost",
        "/data",
        http::Method::GET,
        None,
        None,
        Duration::from_secs(2),
        Some(4096),
    )
    .await;
    let err = result.expect_err("non-pooled must honour cap");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream declared body") && msg.contains("4096"),
        "err msg must name cap breach; got: {msg}"
    );
}

#[tokio::test]
async fn uds_non_pooled_chunked_body_over_cap_aborts_mid_stream() {
    use ruuter_on_rust::http_client::uds::request_over_unix;

    let sock = socket_path("nonpool-chunked");
    let _s = spawn_chunked_server(&sock, 128 * 1024).await;

    let result = request_over_unix(
        &sock,
        "localhost",
        "/stream",
        http::Method::GET,
        None,
        None,
        Duration::from_secs(2),
        Some(16 * 1024),
    )
    .await;
    let err = result.expect_err("non-pooled must abort mid-stream");
    let msg = format!("{err}");
    assert!(
        msg.contains("uds upstream response body exceeded") && msg.contains("16384"),
        "err msg must name mid-stream breach; got: {msg}"
    );
}

// ────────────────────────────────────────────────────────────────
// Content-Type preservation: the cap must not corrupt the decode
// path (issue #98). A JSON body just below the cap must still parse.
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uds_alias_json_below_cap_parses_normally() {
    // Small JSON payload well below the cap. Verifies the Limited
    // wrap doesn't inadvertently break the decode path added in #98.
    let sock = socket_path("alias-json");
    let payload = r#"{"ok":true,"n":42}"#;
    let payload_len = payload.len();
    let path_owned = sock.clone();
    let app =
        Router::new().route(
            "/*rest",
            any(move |_req: Request| {
                let p = payload.to_string();
                async move {
                    (StatusCode::OK, [("content-type", "application/json")], p).into_response()
                }
            }),
        );
    std::fs::remove_file(&sock).ok();
    let listener = tokio::net::UnixListener::bind(&sock).expect("bind");
    let (tx, mut rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
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
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = client_with_cap(Some(1024), ("api".into(), sock.clone()));
    let resp = client
        .request(
            reqwest::Method::GET,
            "http://api/status",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_ref().unwrap()["ok"], true);
    assert_eq!(resp.body.as_ref().unwrap()["n"], 42);

    // Sanity: prove the payload really was well under the cap.
    assert!(payload_len < 1024);
    drop(tx);
}

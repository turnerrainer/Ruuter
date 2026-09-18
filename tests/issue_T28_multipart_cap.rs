//! h2ck.me v1 T-28 — multipart part-count + per-part-size cap.
//!
//! Pre-fix, `parse_multipart_body` iterated every part via
//! `multer::Multipart::next_field()` with no upper bound. A body
//! carrying 10 000 parts allocated a HashMap entry (and buffered
//! the field bytes) per part; a single 500 MB part accumulated
//! into `Vec<u8>` before the caller ever saw it. Cheap DoS
//! amplification against any DSL that accepts multipart —
//! F-PR-2 in `BREAK-TESTS-OWASP-PROBES-v1`.
//!
//! Post-fix, `IncomingRequestsConfig` grows two Optional caps:
//! - `multipart_max_parts` (default `Some(100)`, `None` opts out).
//! - `multipart_max_part_size` (default `Some(4 * 1024 * 1024)`,
//!   `None` opts out).
//! Cap breaches surface as `MultipartError::TooManyParts` /
//! `PartTooLarge` at the parser boundary and map to a structured
//! `413 Payload Too Large` response with an `error` name and the
//! violated `limit` for the caller's diagnostics.
//!
//! Tests written to try to BREAK the fix:
//! - 100 parts + default cap 100 → 200 (right at the boundary).
//! - 101 parts + default cap 100 → 413 with `multipart_too_many_parts`
//!   and `limit: 100`.
//! - 500 parts + default cap 100 → 413 (way over — no hang, no
//!   OOM, no partial buffering signal).
//! - 500 parts + explicit cap = None → 200 (opt-out preserved).
//! - Single part with `max_part_size` cap 1024 and body of 2048
//!   bytes → 413 with `multipart_part_too_large` and `limit: 1024`.
//! - Single part with `max_part_size` cap 1024 and body of 500
//!   bytes → 200 (under the cap).
//! - Malformed multipart frame → still 400 (parser errors are
//!   distinct from cap breaches).

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

/// Build a router with one DSL that echoes `incoming.body` back.
/// `cfg_mut` lets each test flip the multipart caps as needed.
fn build_echo_router<F: FnOnce(&mut AppConfig)>(cfg_mut: F) -> DslRouter {
    let mut cfg = AppConfig::default();
    cfg_mut(&mut cfg);
    let tmp = std::env::temp_dir().join(format!("ruuter-T28-{}", uuid()));
    let dsl_path = tmp.join("svc/POST/echo.yml");
    std::fs::create_dir_all(dsl_path.parent().unwrap()).unwrap();
    std::fs::write(
        &dsl_path,
        r#"
reply:
  return: "${incoming.body}"
  status: 200
"#,
    )
    .unwrap();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(
        HttpClient::new(&cfg),
        ruuter_on_rust::steps::engine::empty_shared_guards(),
        cfg.guards.mode,
    )
    .with_ws_registry(ws.clone())
    .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

const BOUNDARY: &str = "----RuuterT28Boundary";

/// Build a multipart body with `n` fields named "f0..fN-1" and a
/// single "x" byte per part. Each part is ~120 bytes with headers,
/// so 500 parts fits well within the 16 MiB body cap.
fn multipart_with_n_parts(n: usize) -> String {
    let mut body = String::with_capacity(n * 128);
    for i in 0..n {
        body.push_str(&format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"f{i}\"\r\n\r\nx\r\n",
            b = BOUNDARY,
            i = i
        ));
    }
    body.push_str(&format!("--{b}--\r\n", b = BOUNDARY));
    body
}

/// One-part multipart body of exactly `size` payload bytes.
fn multipart_one_part_of_size(size: usize) -> Vec<u8> {
    let header = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"blob\"\r\n\r\n",
        b = BOUNDARY
    );
    let trailer = format!("\r\n--{b}--\r\n", b = BOUNDARY);
    let mut body = Vec::with_capacity(header.len() + size + trailer.len());
    body.extend_from_slice(header.as_bytes());
    body.extend(std::iter::repeat_n(b'a', size));
    body.extend_from_slice(trailer.as_bytes());
    body
}

async fn post_multipart_bytes(router: DslRouter, body: Vec<u8>) -> (StatusCode, serde_json::Value) {
    let app = router.build_axum_router();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={}", BOUNDARY),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("send");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 32 * 1024 * 1024).await.unwrap();
    // Body may not be JSON on 200 for pathological inputs, so
    // return `Null` if parse fails and let callers pattern-match.
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn parts_at_cap_boundary_admits_100() {
    // 100 parts + default cap 100 → 200. `>` not `>=` on the check.
    let router = build_echo_router(|_| {});
    let (status, body) =
        post_multipart_bytes(router, multipart_with_n_parts(100).into_bytes()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "100 parts under default cap 100 must admit; got {} body={}",
        status,
        body
    );
    // Sanity: the response echoes 100 keys.
    let echoed = body["response"].as_object().expect("obj");
    assert_eq!(echoed.len(), 100, "all 100 parts should be echoed");
}

#[tokio::test]
async fn parts_one_over_cap_returns_413() {
    // 101 parts + default cap 100 → 413.
    let router = build_echo_router(|_| {});
    let (status, body) =
        post_multipart_bytes(router, multipart_with_n_parts(101).into_bytes()).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "101 parts must reject at 413; got {} body={}",
        status,
        body
    );
    assert_eq!(body["error"], "multipart_too_many_parts");
    assert_eq!(body["limit"], 100);
}

#[tokio::test]
async fn parts_way_over_cap_returns_413_no_hang() {
    // The named-in-backlog scenario: 500 parts against default cap.
    // Pre-fix, this allocated 500 HashMap entries + buffered bodies.
    // Post-fix, the parser aborts as soon as the counter passes 100.
    let router = build_echo_router(|_| {});
    let (status, body) =
        post_multipart_bytes(router, multipart_with_n_parts(500).into_bytes()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "multipart_too_many_parts");
    assert_eq!(body["limit"], 100);
}

#[tokio::test]
async fn part_size_over_cap_returns_413() {
    // Single part carrying 2048 bytes under a per-part cap of 1024.
    // Pre-fix, the whole part buffered into memory; post-fix the
    // parser aborts mid-stream past 1024.
    let router = build_echo_router(|cfg| {
        cfg.incoming_requests.multipart_max_part_size = Some(1024);
    });
    let (status, body) = post_multipart_bytes(router, multipart_one_part_of_size(2048)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "multipart_part_too_large");
    assert_eq!(body["limit"], 1024);
}

#[tokio::test]
async fn part_size_under_cap_admits() {
    // Sanity: 500 bytes under a 1024 cap round-trips cleanly.
    let router = build_echo_router(|cfg| {
        cfg.incoming_requests.multipart_max_part_size = Some(1024);
    });
    let (status, body) = post_multipart_bytes(router, multipart_one_part_of_size(500)).await;
    assert_eq!(status, StatusCode::OK, "500 bytes under 1024 must admit");
    // The blob is echoed under key "blob" (single ASCII 'a' repeat).
    assert_eq!(body["response"]["blob"], "a".repeat(500));
}

#[tokio::test]
async fn caps_none_preserves_pre_fix_unbounded_behaviour() {
    // Operator explicitly opts out (`None` in ruuter.yaml) — 500
    // parts admits, matching pre-T-28 behaviour for anyone who
    // knows what they're doing.
    let router = build_echo_router(|cfg| {
        cfg.incoming_requests.multipart_max_parts = None;
        cfg.incoming_requests.multipart_max_part_size = None;
    });
    let (status, body) =
        post_multipart_bytes(router, multipart_with_n_parts(500).into_bytes()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "opt-out via caps=None must preserve pre-T-28 behaviour; got {}",
        status
    );
    let echoed = body["response"].as_object().expect("obj");
    assert_eq!(echoed.len(), 500);
}

#[tokio::test]
async fn parse_error_still_returns_400_not_413() {
    // A malformed multipart frame (bad boundary, truncated) is a
    // shape-level parse error, not a policy cap breach — must still
    // be 400 with the "multipart parse:" prefix.
    let router = build_echo_router(|_| {});
    let malformed = b"--WrongBoundary\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n1\r\n";
    let (status, body) = post_multipart_bytes(router, malformed.to_vec()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "malformed multipart must be 400 (not 413 which is reserved for caps); \
         got {} body={}",
        status,
        body
    );
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.starts_with("multipart parse:"),
        "parse error prefix expected; got: {}",
        err
    );
}

/// Regression pin against a subtle miscount — cap = 1 admits
/// exactly one part, rejects the second.
#[tokio::test]
async fn cap_of_one_admits_one_rejects_two() {
    let router = build_echo_router(|cfg| {
        cfg.incoming_requests.multipart_max_parts = Some(1);
    });

    let (s1, b1) = post_multipart_bytes(
        build_echo_router(|cfg| {
            cfg.incoming_requests.multipart_max_parts = Some(1);
        }),
        multipart_with_n_parts(1).into_bytes(),
    )
    .await;
    assert_eq!(s1, StatusCode::OK, "1 part with cap=1 admits");
    assert!(b1["response"]["f0"].as_str().is_some());

    let (s2, b2) = post_multipart_bytes(router, multipart_with_n_parts(2).into_bytes()).await;
    assert_eq!(
        s2,
        StatusCode::PAYLOAD_TOO_LARGE,
        "2 parts with cap=1 rejects"
    );
    assert_eq!(b2["error"], "multipart_too_many_parts");
    assert_eq!(b2["limit"], 1);
}

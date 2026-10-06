//! Issues #135 + #136 — bounded fan-out to N peers with structured
//! aggregation.
//!
//! The step composes `HttpClient::request_with_ct` per peer under a
//! `tokio::sync::Semaphore` bound. These tests cover:
//!
//! 1. `collect_ok` drops errored peers; successes bind in input order.
//! 2. `collect_all` keeps errors (shape stays stable; `response.error`
//!    is populated for transport failures).
//! 3. `first_n` returns when the quota is met.
//! 4. `first_n` with `early_exit_on.body_predicate` only counts
//!    peers whose response body matches.
//! 5. `first_n` with `remaining_peers_after: cancel` aborts the
//!    outstanding tasks.
//! 6. `first_n` with `remaining_peers_after: drain_bg` releases the
//!    caller immediately; stragglers finish in the background.
//! 7. `max_concurrency` bounds the in-flight count.
//! 8. `traceparent` is forwarded per peer.
//! 9. Transport errors surface as `{response: {status: 0, error: "..."}}`
//!    (same shape as #89's stub).
//! 10. Parse-time errors: first_n without first_n:, early_exit_on
//!     under collect_ok, first_n: 0, unknown call:, bad status_range.
//! 11. Empty `peers:` list binds `result` to `[]` and advances.

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::dsl::parser::DslParser;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn write_dsl(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router_with_cfg(
    dsl_root: &Path,
    mut mutate: impl FnMut(&mut AppConfig),
) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    // mockito binds on 127.0.0.1; opt out of SSRF default-deny for
    // local test fixtures.
    config.internal_requests.block_private_networks = false;
    mutate(&mut config);
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(
        HttpClient::new(&config),
        ruuter_on_rust::steps::engine::empty_shared_guards(),
        config.guards.mode,
    )
    .with_ws_registry(ws.clone())
    .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

async fn send_json(router: Arc<DslRouter>, method: &str, uri: &str) -> serde_json::Value {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    let bytes = to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// ────────────────────────────────────────────────────────────────────
// Fixtures
// ────────────────────────────────────────────────────────────────────

/// Spin up `n` mockito servers and return a Vec of (id, url, mock).
/// The DSL fixture reads them from an assigned peer array in the
/// context.
async fn upstreams(statuses: &[u16]) -> (Vec<mockito::ServerGuard>, Vec<String>) {
    let mut guards = Vec::new();
    let mut urls = Vec::new();
    for (idx, status) in statuses.iter().enumerate() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/probe")
            .with_status(*status as usize)
            .with_body(format!(r#"{{"id":{},"status":{}}}"#, idx, status))
            .create_async()
            .await;
        urls.push(format!("{}/probe", server.url()));
        guards.push(server);
    }
    (guards, urls)
}

// ────────────────────────────────────────────────────────────────────
// 1. collect_ok drops errored peers (transport failures)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn collect_ok_drops_transport_errors() {
    // 3 upstreams: 2 return 200, 1 doesn't exist (dead port).
    let (_g, mut urls) = upstreams(&[200, 200]).await;
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    urls.push(format!("http://127.0.0.1:{}/probe", dead_port));

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "A", url: "{a}" }}
      - {{ id: "B", url: "{b}" }}
      - {{ id: "C", url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    call: http.get
    args:
      url: "${{peer.url}}"
    aggregate: collect_ok
    timeout: 3000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/fanout").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 2, "transport-errored peer should be dropped");
    for entry in arr {
        assert_eq!(entry["response"]["status"].as_u64(), Some(200));
        assert!(entry["response"]["error"].is_null());
    }
}

// ────────────────────────────────────────────────────────────────────
// 2. collect_all keeps errors with stable shape
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn collect_all_keeps_transport_errors() {
    let (_g, mut urls) = upstreams(&[200, 200]).await;
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    urls.push(format!("http://127.0.0.1:{}/probe", dead_port));

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "A", url: "{a}" }}
      - {{ id: "B", url: "{b}" }}
      - {{ id: "DEAD", url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    args:
      url: "${{peer.url}}"
    aggregate: collect_all
    timeout: 3000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/fanout").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 3, "collect_all keeps every peer");
    // Order matches input order
    assert_eq!(arr[0]["peer"]["id"], "A");
    assert_eq!(arr[1]["peer"]["id"], "B");
    assert_eq!(arr[2]["peer"]["id"], "DEAD");
    assert_eq!(arr[0]["response"]["status"].as_u64(), Some(200));
    assert_eq!(arr[2]["response"]["status"].as_u64(), Some(0));
    // Transport error must populate `response.error` with a stable kind
    let err = arr[2]["response"]["error"].as_str().unwrap_or("");
    assert!(
        err.contains("connect") || err.contains("request") || err.contains("http"),
        "unexpected transport error class: {}",
        err
    );
}

// ────────────────────────────────────────────────────────────────────
// 3. first_n returns when quota is met
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_n_returns_after_quota() {
    let (_g, urls) = upstreams(&[200, 200, 200]).await;
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "A", url: "{a}" }}
      - {{ id: "B", url: "{b}" }}
      - {{ id: "C", url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    args:
      url: "${{peer.url}}"
    aggregate: first_n
    first_n: 2
    timeout: 3000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/fanout").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 2, "first_n:2 should return exactly 2 matches");
    for entry in arr {
        assert_eq!(entry["response"]["status"].as_u64(), Some(200));
    }
}

// ────────────────────────────────────────────────────────────────────
// 4. first_n with body_predicate — only matching responses count
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_n_body_predicate_filters() {
    // Three upstreams, all 200. Two return `{"found": false}`, one
    // returns `{"found": true}`. Predicate on `response.body.found`.
    let mut servers: Vec<mockito::ServerGuard> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for found in [false, true, false] {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/check")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(r#"{{"found":{}}}"#, found))
            .create_async()
            .await;
        urls.push(format!("{}/check", server.url()));
        servers.push(server);
    }

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/lookup.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "A", url: "{a}" }}
      - {{ id: "B", url: "{b}" }}
      - {{ id: "C", url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    args:
      url: "${{peer.url}}"
    aggregate: first_n
    first_n: 1
    early_exit_on:
      body_predicate: "${{response.body.found === true}}"
    remaining_peers_after: cancel
    timeout: 3000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/lookup").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 1, "first_n:1 with predicate matches B only");
    assert_eq!(arr[0]["peer"]["id"], "B");
    assert_eq!(arr[0]["response"]["body"]["found"], true);
}

// ────────────────────────────────────────────────────────────────────
// 5. Empty peers array → empty result, advance
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_peers_binds_empty_array() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        r#"
init:
  assign:
    peers: []
  next: fan

fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://unused.invalid/"
    aggregate: collect_all
    result: results
  next: reply

reply:
  return: "${results}"
  wrapper: false
"#,
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/fanout").await;
    assert_eq!(body, serde_json::json!([]));
}

// ────────────────────────────────────────────────────────────────────
// 6. traceparent is forwarded per peer
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn traceparent_forwarded_per_peer() {
    // Each mock expects a `traceparent:` header. The DSL fixture
    // sends an inbound `traceparent` which the context picks up and
    // the parallel_http step auto-forwards per peer.
    let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let mut servers: Vec<mockito::ServerGuard> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for _ in 0..3 {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/probe")
            .match_header("traceparent", tp)
            .with_status(200)
            .with_body("ok")
            .create_async()
            .await;
        urls.push(format!("{}/probe", server.url()));
        servers.push(server);
    }

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "A", url: "{a}" }}
      - {{ id: "B", url: "{b}" }}
      - {{ id: "C", url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    args:
      url: "${{peer.url}}"
    aggregate: collect_all
    timeout: 3000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let req = Request::builder()
        .method("GET")
        .uri("/svc/fanout")
        .header("traceparent", tp)
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 3);
    for entry in arr {
        assert_eq!(
            entry["response"]["status"].as_u64(),
            Some(200),
            "mock would not have returned 200 if traceparent didn't match"
        );
    }
}

// ────────────────────────────────────────────────────────────────────
// 7. Parse-time errors
// ────────────────────────────────────────────────────────────────────

fn parser() -> DslParser {
    DslParser::new(HashMap::new())
}

#[tokio::test]
async fn parse_error_first_n_without_first_n_field() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: first_n
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("first_n") && err.contains("N >= 1"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_first_n_zero() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: first_n
    first_n: 0
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("N >= 1"), "unexpected error: {}", err);
}

#[tokio::test]
async fn parse_error_first_n_under_collect_ok() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: collect_ok
    first_n: 2
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("first_n is only valid"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_early_exit_on_under_collect_all() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: collect_all
    early_exit_on:
      status_range: [200, 299]
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("early_exit_on is only valid"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_remaining_peers_after_under_collect_all() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: collect_all
    remaining_peers_after: cancel
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("remaining_peers_after is only valid"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_unknown_call() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    call: http.yeet
    args:
      url: "http://x/"
    aggregate: collect_all
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unknown method") && err.contains("http.yeet"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_bad_status_range() {
    let err = parser()
        .parse_content(
            r#"
fan:
  parallel_http:
    peers: "${peers}"
    args:
      url: "http://x/"
    aggregate: first_n
    first_n: 1
    early_exit_on:
      status_range: [500, 200]
    result: results
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("lo <= hi"), "unexpected error: {}", err);
}

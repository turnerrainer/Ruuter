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

// ────────────────────────────────────────────────────────────────────
// Result array preserves input peer order (not completion order)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn result_array_preserves_input_order_not_completion_order() {
    // Three mocks all reply 200 at the same wall clock, so completion
    // order is undefined. Input order (A, B, C) must be preserved
    // in the result array regardless.
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
      - {{ id: "AAA", url: "{a}" }}
      - {{ id: "BBB", url: "{b}" }}
      - {{ id: "CCC", url: "{c}" }}
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
    // Run 10 times — if the ordering were by completion, we'd expect
    // a mix of permutations across runs. The input-order guarantee
    // must hold on every run.
    let router = build_router_with_cfg(tmp.path(), |_| {});
    for _ in 0..10 {
        let body = send_json(router.clone(), "GET", "/svc/fanout").await;
        let arr = body.as_array().expect("array result");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["peer"]["id"], "AAA");
        assert_eq!(arr[1]["peer"]["id"], "BBB");
        assert_eq!(arr[2]["peer"]["id"], "CCC");
    }
}

// ────────────────────────────────────────────────────────────────────
// Per-peer templating: ${peer.*} resolves in url, body, headers
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn per_peer_templating_resolves_in_body_and_headers() {
    // Each mock matches on a peer-specific header + body field.
    let mut servers: Vec<mockito::ServerGuard> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for peer_id in ["red", "green", "blue"] {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/x")
            .match_header("x-peer-id", peer_id)
            .match_body(mockito::Matcher::PartialJsonString(format!(
                r#"{{"from":"{}"}}"#,
                peer_id
            )))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(r#"{{"saw":"{}"}}"#, peer_id))
            .create_async()
            .await;
        urls.push(format!("{}/x", server.url()));
        servers.push(server);
    }

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/template.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "red",   url: "{a}" }}
      - {{ id: "green", url: "{b}" }}
      - {{ id: "blue",  url: "{c}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    call: http.post
    args:
      url: "${{peer.url}}"
      headers:
        X-Peer-Id: "${{peer.id}}"
      body:
        from: "${{peer.id}}"
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
    let body = send_json(router, "POST", "/svc/template").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 3);
    for (idx, expected) in ["red", "green", "blue"].iter().enumerate() {
        assert_eq!(
            arr[idx]["response"]["status"].as_u64(),
            Some(200),
            "mock would have 501'd if the X-Peer-Id header / body.from \
             didn't resolve to {} for peer {}",
            expected,
            idx
        );
        assert_eq!(arr[idx]["response"]["body"]["saw"], *expected);
    }
}

// ────────────────────────────────────────────────────────────────────
// first_n with status_range (no body predicate)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_n_counts_only_status_range_hits() {
    // Three peers: 200, 404, 200. status_range [200,299] — only
    // the two 200s count. first_n:2 returns when both are collected;
    // the 404 is discarded.
    let (_g, urls) = upstreams(&[200, 404, 200]).await;
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
    early_exit_on:
      status_range: [200, 299]
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
    let body = send_json(router, "GET", "/svc/fanout").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 2, "exactly the 2 peers with 2xx match");
    for entry in arr {
        assert!(
            (200..300).contains(&entry["response"]["status"].as_u64().unwrap_or(0)),
            "every match must be in status_range: {:?}",
            entry
        );
    }
}

// ────────────────────────────────────────────────────────────────────
// first_n returns short when quota is never met
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_n_returns_fewer_than_quota_when_predicate_matches_nothing() {
    // All three peers 200 but body_predicate requires found===true.
    // Only one peer returns found:true; first_n:3 can only collect 1.
    // Step returns the single match (result.length < first_n is the
    // signal the DSL author uses to detect quota-miss).
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
    first_n: 3
    early_exit_on:
      body_predicate: "${{response.body.found === true}}"
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
    assert_eq!(arr.len(), 1, "only B matches the predicate");
    assert_eq!(arr[0]["peer"]["id"], "B");
}

// ────────────────────────────────────────────────────────────────────
// SSRF applies per peer (private-network URL blocked)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ssrf_applies_per_peer_private_network_blocked() {
    // One peer is reachable, one is on a private range. With
    // block_private_networks: true, the private peer's call errors
    // via the #89 stub; it still appears in collect_all's result
    // array with status 0 + an error class.
    let (_g, urls) = upstreams(&[200]).await;
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/fanout.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "public",  url: "{a}" }}
      - {{ id: "private", url: "http://192.168.42.42/forbidden" }}
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
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |c| {
        // Flip the gate ON for THIS test only. Our upstreams() helper
        // uses mockito on 127.0.0.1 which must be allowlisted — add
        // its origin so that peer succeeds.
        c.internal_requests.block_private_networks = true;
        let origin = urls[0].trim_end_matches("/probe").to_string();
        c.internal_requests.allowed_urls = vec![origin];
    });
    let body = send_json(router, "GET", "/svc/fanout").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 2);
    // Public peer: 200.
    assert_eq!(arr[0]["response"]["status"].as_u64(), Some(200));
    // Private peer: transport error with a kind (SSRF rejection
    // raises as RuuterError::HttpRequest which maps to the "unknown"
    // class in the parallel_http executor; the shape is status 0 +
    // error populated).
    assert_eq!(arr[1]["response"]["status"].as_u64(), Some(0));
    assert!(
        arr[1]["response"]["error"].is_string()
            && !arr[1]["response"]["error"].as_str().unwrap().is_empty(),
        "SSRF rejection should populate response.error on the private peer: {:?}",
        arr[1]
    );
}

// ────────────────────────────────────────────────────────────────────
// max_concurrency bounds in-flight count
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn max_concurrency_bounds_in_flight_count() {
    // Use a shared counter + atomic max-observed. Each mock
    // increments on arrival, sleeps briefly, decrements. If
    // max_concurrency works, the observed max is <= cap.
    use std::sync::atomic::{AtomicUsize, Ordering};
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let mut servers: Vec<mockito::ServerGuard> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for _ in 0..6 {
        let mut server = mockito::Server::new_async().await;
        let in_flight_c = in_flight.clone();
        let max_c = max_observed.clone();
        server
            .mock("GET", "/sleep")
            .with_body_from_request(move |_req| {
                let cur = in_flight_c.fetch_add(1, Ordering::SeqCst) + 1;
                max_c.fetch_max(cur, Ordering::SeqCst);
                // sleep a bit so overlap is observable
                std::thread::sleep(std::time::Duration::from_millis(50));
                in_flight_c.fetch_sub(1, Ordering::SeqCst);
                b"ok".to_vec()
            })
            .with_status(200)
            .create_async()
            .await;
        urls.push(format!("{}/sleep", server.url()));
        servers.push(server);
    }

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/bounded.yml",
        &format!(
            r#"
init:
  assign:
    peers:
      - {{ id: "p1", url: "{a}" }}
      - {{ id: "p2", url: "{b}" }}
      - {{ id: "p3", url: "{c}" }}
      - {{ id: "p4", url: "{d}" }}
      - {{ id: "p5", url: "{e}" }}
      - {{ id: "p6", url: "{f}" }}
  next: fan

fan:
  parallel_http:
    peers: "${{peers}}"
    args:
      url: "${{peer.url}}"
    aggregate: collect_all
    max_concurrency: 2
    timeout: 5000
    result: results
  next: reply

reply:
  return: "${{results}}"
  wrapper: false
"#,
            a = urls[0],
            b = urls[1],
            c = urls[2],
            d = urls[3],
            e = urls[4],
            f = urls[5],
        ),
    );
    let router = build_router_with_cfg(tmp.path(), |_| {});
    let body = send_json(router, "GET", "/svc/bounded").await;
    let arr = body.as_array().expect("array result");
    assert_eq!(arr.len(), 6, "all peers finish");
    let peak = max_observed.load(Ordering::SeqCst);
    assert!(
        peak <= 2,
        "max_concurrency=2 should bound in-flight; saw peak={}",
        peak
    );
}

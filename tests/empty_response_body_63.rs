//! Issue #63 — empty upstream response body must surface as an
//! empty string, not JSON `null`; a `Value::Null` forwarded as a
//! plaintext outbound body must serialise as empty on the wire, not
//! the literal string `"null"`.
//!
//! Two halves, tested through mockito upstreams:
//!
//! 1. **Bind**: an `http.get` against an endpoint returning
//!    `text/plain; content-length: 0` must bind `${result.response.body}`
//!    as `Value::String("")`. Under the pre-#63 code it bound as
//!    `Value::Null`, which surfaced under a `return:` wrapper as
//!    `"body": null` and confused DSLs downstream.
//!
//! 2. **Forward**: taking a `Value::Null` and passing it as a
//!    plaintext body to another `http.<verb>` step must send an
//!    empty wire body, not the four bytes `n-u-l-l`. Defensive fix
//!    for any code path where `null` slips into the outbound body
//!    slot (not just the empty-body case, which #63's part 1 fixes
//!    at source).

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
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

fn write_dsl(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn build_router(root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ))
}

async fn get(router: Arc<DslRouter>, path: &str) -> (u16, String) {
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Part 1: empty upstream body binds as `""` under
/// `${result.response.body}`, not `null`.
#[tokio::test]
async fn empty_upstream_body_binds_as_empty_string_not_null() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/empty")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/empty"
  result: r
  next: reply

reply:
  return:
    body_is: "${{typeof r.response.body}}"
    body_value: "${{r.response.body}}"
  status: 200
"#,
            server.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/probe").await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    // The response is envelope-wrapped: `{"response": {...}}`
    let inner = &parsed["response"];
    assert_eq!(
        inner["body_is"], "string",
        "empty upstream body must be a string, not null; got: {body}"
    );
    assert_eq!(
        inner["body_value"], "",
        "empty upstream body must be empty string; got: {body}"
    );
}

/// Part 2 (end-to-end): fetching an empty upstream body and
/// forwarding it as plaintext to another endpoint must send an
/// empty wire body — not `"null"`.
#[tokio::test]
async fn empty_upstream_body_forwarded_as_plaintext_is_empty_on_wire() {
    let mut upstream = mockito::Server::new_async().await;
    let _upstream_mock = upstream
        .mock("GET", "/upstream/empty")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    // Downstream mock captures what body arrives. Set a matcher on
    // "" (exact empty body) so if the fix regresses (body arrives as
    // "null"), the mock's expectation fails and the assertion below
    // catches it.
    let mut downstream = mockito::Server::new_async().await;
    let downstream_mock = downstream
        .mock("POST", "/downstream/echo")
        .match_body(mockito::Matcher::Exact(String::new()))
        .with_status(200)
        .with_body("received")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/relay.yml",
        &format!(
            r#"
fetch_upstream:
  call: http.get
  args:
    url: "{upstream}/upstream/empty"
  result: r
  next: forward

forward:
  call: http.post
  args:
    url: "{downstream}/downstream/echo"
    body: "${{r.response.body}}"
    content_type: "plaintext"
  result: dsr
  next: reply

reply:
  return:
    forwarded: "${{dsr.response.body}}"
  status: 200
"#,
            upstream = upstream.url(),
            downstream = downstream.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/relay").await;
    assert_eq!(status, 200);
    // If the downstream mock's Exact("") body matcher didn't match,
    // mockito wouldn't record a hit — so the assertion below covers
    // BOTH the "wire body was empty" contract AND the "we got a
    // response through" contract.
    downstream_mock.assert_async().await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["forwarded"], "received");
}

/// Part 2 (defensive): even without going through an http round
/// trip, a Value::Null forwarded directly as a plaintext body must
/// serialise as empty. Covers any other code path where a null slips
/// into the outbound body slot (not just the #63 upstream case).
#[tokio::test]
async fn null_body_forwarded_as_plaintext_is_empty_on_wire() {
    let mut downstream = mockito::Server::new_async().await;
    let downstream_mock = downstream
        .mock("POST", "/plain/echo")
        .match_body(mockito::Matcher::Exact(String::new()))
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/nullbody.yml",
        &format!(
            r#"
prep:
  assign:
    payload: "${{null}}"
  next: send

send:
  call: http.post
  args:
    url: "{}/plain/echo"
    body: "${{payload}}"
    content_type: "plaintext"
  result: r
  next: reply

reply:
  return:
    got: "${{r.response.body}}"
  status: 200
"#,
            downstream.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/nullbody").await;
    assert_eq!(status, 200);
    downstream_mock.assert_async().await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["got"], "ok");
}

/// Regression baseline: a non-empty text/plain upstream body still
/// binds correctly as a Value::String (issue #23 preserved).
#[tokio::test]
async fn non_empty_plaintext_upstream_still_binds_as_string() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/greeting")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("hello world")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/hi.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/greeting"
  result: r
  next: reply

reply:
  return:
    body: "${{r.response.body}}"
  status: 200
"#,
            server.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/hi").await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["body"], "hello world");
}

/// Empty body under `content-type: application/json` (technically
/// invalid — empty JSON — but real APIs do this). Same binding as
/// text/plain: `""`, not JSON `null`. Documents the content-type-
/// agnostic contract of fix (a).
#[tokio::test]
async fn empty_upstream_body_under_json_content_type_also_binds_as_string() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/empty-json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe-json.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/empty-json"
  result: r
  next: reply

reply:
  return:
    body_type: "${{typeof r.response.body}}"
    body_value: "${{r.response.body}}"
  status: 200
"#,
            server.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/probe-json").await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    let inner = &parsed["response"];
    assert_eq!(
        inner["body_type"], "string",
        "empty JSON upstream must bind as string, not null: {body}"
    );
    assert_eq!(
        inner["body_value"], "",
        "empty JSON upstream must bind as empty string: {body}"
    );
}

/// Chained round-trip: DSL A fetches upstream (empty body), forwards
/// through DSL B as plaintext, DSL B forwards through DSL C as
/// plaintext, DSL C's outbound wire body must still be empty. Guards
/// against a fix regression only visible after multiple hops.
#[tokio::test]
async fn chained_forward_of_empty_upstream_body_stays_empty() {
    let mut hop1 = mockito::Server::new_async().await;
    let _u = hop1
        .mock("GET", "/origin/empty")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    let mut hop2 = mockito::Server::new_async().await;
    // First mid-hop mock echoes the body it received (as its own
    // response body). If the body arrived non-empty, mock returns
    // it verbatim; if empty, mock returns "". Either way our
    // downstream (hop3) does the final Exact("") check.
    let _mid = hop2
        .mock("POST", "/mid/echo")
        .match_body(mockito::Matcher::Exact(String::new()))
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    let mut hop3 = mockito::Server::new_async().await;
    let final_mock = hop3
        .mock("POST", "/dest/receive")
        .match_body(mockito::Matcher::Exact(String::new()))
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/chain.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{origin}/origin/empty"
  result: a
  next: hop1

hop1:
  call: http.post
  args:
    url: "{mid}/mid/echo"
    body: "${{a.response.body}}"
    content_type: "plaintext"
  result: b
  next: hop2

hop2:
  call: http.post
  args:
    url: "{final_url}/dest/receive"
    body: "${{b.response.body}}"
    content_type: "plaintext"
  result: c
  next: reply

reply:
  return:
    final: "${{c.response.body}}"
  status: 200
"#,
            origin = hop1.url(),
            mid = hop2.url(),
            final_url = hop3.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/chain").await;
    assert_eq!(status, 200);
    final_mock.assert_async().await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["final"], "ok");
}

/// `${result.response.body}` from a genuinely-null value (produced by
/// a `${null}` assign, not by an empty upstream) forwarded under
/// `content_type: "plaintext"` must still be empty on the wire.
/// Regression guard for fix (b), independent of fix (a)'s upstream
/// path.
#[tokio::test]
async fn explicit_null_expression_forwarded_as_plaintext_is_empty() {
    let mut downstream = mockito::Server::new_async().await;
    let m = downstream
        .mock("POST", "/plain/exact")
        .match_body(mockito::Matcher::Exact(String::new()))
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/nullexpr.yml",
        &format!(
            r#"
send:
  call: http.post
  args:
    url: "{}/plain/exact"
    body: "${{null}}"
    content_type: "plaintext"
  result: r
  next: reply

reply:
  return:
    got: "${{r.response.body}}"
  status: 200
"#,
            downstream.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/nullexpr").await;
    assert_eq!(status, 200);
    m.assert_async().await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["got"], "ok");
}

/// Whitespace-only upstream body (not empty, but content-length > 0
/// with only spaces) must bind as the whitespace string, NOT be
/// coerced to empty. Guards against an over-eager fix that treats
/// any "empty-ish" content as `""`.
#[tokio::test]
async fn whitespace_only_upstream_body_binds_verbatim() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/spaces")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("   ")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/ws.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/spaces"
  result: r
  next: reply

reply:
  return:
    body: "${{r.response.body}}"
    length: "${{r.response.body.length}}"
  status: 200
"#,
            server.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/ws").await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["response"]["body"], "   ");
    assert_eq!(parsed["response"]["length"], 3);
}

/// Behaviour-change guard: pre-#63 a DSL that checked
/// `${r.response.body === null}` would fire on empty. Now the body
/// is `""` and that strict-equality check flips. This test locks
/// down the new state so a reviewer can see the change in the
/// diff (rather than in a downstream DSL breaking silently).
///
/// Uses an explicit `.length > 0` check that returns a real boolean,
/// so this test is independent of the #64 truthy-switch change.
#[tokio::test]
async fn empty_body_is_empty_string_not_null_via_length_check() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/empty")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_header("content-length", "0")
        .with_body("")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/branch.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/empty"
  result: r
  next: pick

pick:
  switch:
    - condition: "${{r.response.body.length > 0}}"
      next: has_data
  next: no_data

has_data:
  return: {{ picked: "has_data" }}
  next: end

no_data:
  return: {{ picked: "no_data" }}
  next: end
"#,
            server.url()
        ),
    );

    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/branch").await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    // Under pre-#63 (body = null), `null.length` throws TypeError → whole
    // step errors. Under #63, body = "" and `"".length === 0` → switch
    // falls through cleanly.
    assert_eq!(
        parsed["response"]["picked"], "no_data",
        "empty body must be a real string with length property (not null): {body}"
    );

    // Non-empty upstream: body is "hi", length > 0, matches.
    let mut server2 = mockito::Server::new_async().await;
    let _mock2 = server2
        .mock("GET", "/hi")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("hi")
        .create_async()
        .await;

    let tmp2 = TempDir::new().unwrap();
    write_dsl(
        tmp2.path(),
        "svc/GET/branch2.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{}/hi"
  result: r
  next: pick

pick:
  switch:
    - condition: "${{r.response.body.length > 0}}"
      next: has_data
  next: no_data

has_data:
  return: {{ picked: "has_data" }}
  next: end

no_data:
  return: {{ picked: "no_data" }}
  next: end
"#,
            server2.url()
        ),
    );

    let router2 = build_router(tmp2.path());
    let (status2, body2) = get(router2, "/svc/branch2").await;
    assert_eq!(status2, 200);
    let parsed2: serde_json::Value = serde_json::from_str(&body2).expect("valid JSON");
    assert_eq!(parsed2["response"]["picked"], "has_data");
}

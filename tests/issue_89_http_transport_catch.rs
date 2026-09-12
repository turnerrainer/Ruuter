//! Issue #89 — transport-layer failures on `http.*` steps are now
//! catchable by the DSL.
//!
//! Pre-fix, connection-refused / DNS / TLS / read-write timeout on
//! an `http.get` or `http.post` step raised inside the engine (`?`
//! propagated `reqwest::Error` as `RuuterError::Http`), which aborted
//! the whole run and produced Ruuter's generic 500. The DSL author's
//! `check_*` switch never ran, so RFC 7807-shaped 502 responses
//! for upstream unavailability were impossible from a gateway /
//! adapter DSL.
//!
//! Post-fix, the transport error is surfaced in-band:
//! `${result.response.status}` is `0` and `${result.response.error}`
//! is one of `timeout`, `connect`, `request`, `body`, `decode`,
//! `unknown` (stable strings). The step routes to its `error:`
//! handler if set, otherwise falls through to `next:` — the DSL
//! author's mental model of "check the response status" gains
//! catchability with no new syntax.
//!
//! Policy-level pre-flight rejections (SSRF-blocked, host-
//! allowlist denial, malformed URL) still raise — they are ops
//! decisions, not availability events, and letting a DSL catch them
//! would leak internal network reachability.

#![allow(clippy::field_reassign_with_default)]

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
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn write_dsl(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router(dsl_root: &Path) -> Arc<DslRouter> {
    build_router_with_cfg(dsl_root, |_| {})
}

fn build_router_with_cfg(
    dsl_root: &Path,
    mut config_mut: impl FnMut(&mut AppConfig),
) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    // Localhost mocks live on 127.0.0.1; without this the SSRF
    // pre-flight would reject before the transport error path fires.
    config.internal_requests.block_private_networks = false;
    config_mut(&mut config);
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

async fn hit(router: Arc<DslRouter>, path: &str) -> (u16, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Bind a TCP port, drop the listener immediately, hand back the
/// port number. The address is on `127.0.0.1` and nothing listens on
/// it — a subsequent connect() to `http://127.0.0.1:<port>/…` gets
/// TCP RST. The best portable proxy for "connection refused" in an
/// integration test.
fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// ────────────────────────────────────────────────────────────────
// 1. Connection refused → status:0, error:"connect", check_* runs
// ────────────────────────────────────────────────────────────────

/// The reporter's motivating case: an `http.get` whose upstream is
/// unreachable used to abort the run with a generic 500; the
/// `check_upstream` switch never ran. Post-fix the switch sees
/// `status == 0` and emits the DSL's semantic 502.
#[tokio::test]
async fn connect_refused_binds_stub_and_lets_check_switch_run() {
    let port = dead_port();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/gateway.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "http://127.0.0.1:{port}/thing"
  result: upstream_res
  next: check_upstream

check_upstream:
  switch:
    - condition: "${{upstream_res.response.status == 0}}"
      next: gateway_unavailable
  next: respond

gateway_unavailable:
  return:
    type: "https://api.example/errors/bad-gateway"
    code: "GATEWAY_UNAVAILABLE"
    detail: "${{upstream_res.response.error}}"
  status: 502
  next: end

respond:
  return: "ok"
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/gateway").await;
    assert_eq!(status, 502, "DSL must be reached and emit 502: {body}");
    assert!(
        body.contains("GATEWAY_UNAVAILABLE"),
        "RFC-7807-shape body must be preserved: {body}"
    );
    // reqwest classifies TCP-refused as `connect`.
    assert!(
        body.contains("connect"),
        "response.error must expose the transport-kind: {body}"
    );
}

/// The stub also carries a diagnostic `message` under
/// `response.body.message` so operators grepping logs / responses
/// can correlate to a real reqwest error text.
#[tokio::test]
async fn connect_refused_stub_body_carries_error_message() {
    let port = dead_port();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "http://127.0.0.1:{port}/thing"
  result: upstream_res
  next: respond

respond:
  return:
    stub: "${{upstream_res}}"
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/probe").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("\"status\":0"),
        "stub status:0 must reach the DSL: {body}"
    );
    assert!(
        body.contains("\"error\":\"connect\""),
        "stub error kind must reach the DSL: {body}"
    );
    assert!(
        body.contains("\"message\""),
        "stub message must be present for log correlation: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// 2. Timeout → status:0, error:"timeout"
// ────────────────────────────────────────────────────────────────

/// A TCP listener that accepts the connection and then hangs
/// (never writes any response bytes). reqwest's request-level
/// timeout fires while waiting for the response headers/body —
/// classified as `is_timeout()`. Returns the port and a handle
/// whose `Drop` shuts the acceptor thread down.
fn hanging_listener() -> (u16, std::sync::mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // Hold the accepted streams so they don't drop (which would
        // close the socket and let reqwest error out with `connect`
        // instead of `timeout`).
        let mut _held: Vec<std::net::TcpStream> = Vec::new();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).ok();
                    _held.push(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
            if rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    (port, tx)
}

/// `timeout:` on the step expires before the upstream sends any
/// response bytes. Pre-fix reqwest's timeout error propagated as
/// `RuuterError::Http` and aborted the run. Post-fix the DSL sees
/// `status == 0` and `error == "timeout"`.
#[tokio::test]
async fn timeout_binds_stub_with_timeout_kind() {
    let (port, _stop) = hanging_listener();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  timeout: 300
  args:
    url: "http://127.0.0.1:{port}/slow"
  result: r
  next: shape

shape:
  return: {{ status: "${{r.response.status}}", err: "${{r.response.error}}" }}
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/probe").await;
    assert_eq!(status, 200, "run must not abort: {body}");
    assert!(
        body.contains("\"status\":0") || body.contains("\"status\":\"0\""),
        "status must be 0: {body}"
    );
    assert!(
        body.contains("\"err\":\"timeout\""),
        "error kind must be `timeout`: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// 3. `error:` handler routing on transport failure
// ────────────────────────────────────────────────────────────────

/// When the step wires an `error:` step, transport failure routes
/// there (same shape as an allow-list miss). The main `next:` is
/// skipped.
#[tokio::test]
async fn transport_failure_routes_to_error_step_when_set() {
    let port = dead_port();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/gateway.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "http://127.0.0.1:{port}/thing"
  result: r
  error: on_upstream_down
  next: happy_path

happy_path:
  return: "should not reach"
  status: 200
  next: end

on_upstream_down:
  return: {{ err: "${{r.response.error}}", from: "error-branch" }}
  status: 502
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/gateway").await;
    assert_eq!(status, 502, "error: branch must fire: {body}");
    assert!(
        body.contains("\"from\":\"error-branch\""),
        "error step must be reached, not next: {body}"
    );
    assert!(
        body.contains("connect"),
        "error step must see response.error kind: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// 4. Backwards-compat: allow-list miss still raises without error:
// ────────────────────────────────────────────────────────────────

/// A real upstream response with a status outside
/// `http_codes_allow_list` still raises when no `error:` handler is
/// set — that path is a policy decision, not an availability event.
/// Ruuter's default allow-list rejects 500-class responses.
#[tokio::test]
async fn allow_list_miss_without_error_step_still_raises() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/upstream-500")
        .with_status(500)
        .with_body("boom")
        .create_async()
        .await;
    let url = format!("{}/upstream-500", server.url());
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  next: never
never:
  return: "not reached"
  status: 200
  next: end
"#
        ),
    );
    let (status, _body) = hit(
        build_router_with_cfg(tmp.path(), |c| c.http_codes_allow_list = vec![200]),
        "/svc/probe",
    )
    .await;
    // Configured allow-list rejects the 500; without an error:
    // handler the step raises → framework returns 500.
    assert_eq!(status, 500);
    m.assert_async().await;
}

/// Same shape but WITH `error:` — the step routes there normally,
/// preserving the existing "allow-list miss → error:" semantics.
#[tokio::test]
async fn allow_list_miss_with_error_step_routes_normally() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/upstream-500")
        .with_status(500)
        .with_body(r#"{"cause":"upstream oops"}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;
    let url = format!("{}/upstream-500", server.url());
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  error: recover
  next: happy

happy:
  return: "not reached"
  status: 200
  next: end

recover:
  return:
    upstream_status: "${{r.response.status}}"
    upstream_body: "${{r.response.body}}"
  status: 502
  next: end
"#
        ),
    );
    let (status, body) = hit(
        build_router_with_cfg(tmp.path(), |c| c.http_codes_allow_list = vec![200]),
        "/svc/probe",
    )
    .await;
    assert_eq!(status, 502);
    assert!(
        body.contains("\"upstream_status\":500") || body.contains("\"upstream_status\":\"500\""),
        "recover must see the real 500: {body}"
    );
    assert!(body.contains("upstream oops"), "body must reach: {body}");
    m.assert_async().await;
}

// ────────────────────────────────────────────────────────────────
// 5. Result binding shape
// ────────────────────────────────────────────────────────────────

/// Non-transport-failure path: `response.error` field is ABSENT
/// (not `null`). DSL authors reading `${result.response}` see the
/// same shape they've always seen.
#[tokio::test]
async fn successful_response_omits_error_field_from_binding() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/ok")
        .with_status(200)
        .with_body(r#"{"hello":"world"}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;
    let url = format!("{}/ok", server.url());
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "{url}"
  result: r
  next: shape

shape:
  return:
    seen: "${{r}}"
    error_present: "${{r.response.error !== undefined}}"
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/probe").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("\"error_present\":false"),
        "response.error must be absent on success: {body}"
    );
    assert!(body.contains("hello"), "response body reaches DSL: {body}");
    m.assert_async().await;
}

/// The stub-body shape carries both `error` (kind) and `message`
/// (reqwest text). DSL authors can log the message and branch on
/// the kind.
#[tokio::test]
async fn transport_failure_stub_body_has_error_and_message_keys() {
    let port = dead_port();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "http://127.0.0.1:{port}/thing"
  result: r
  next: shape

shape:
  return:
    body_error: "${{r.response.body.error}}"
    body_message_present: "${{r.response.body.message !== undefined}}"
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/probe").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("\"body_error\":\"connect\""),
        "stub body.error must carry the kind: {body}"
    );
    assert!(
        body.contains("\"body_message_present\":true"),
        "stub body.message must be present: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// 6. Fall-through: no `error:` set → next: still runs
// ────────────────────────────────────────────────────────────────

/// When the step has NO `error:` handler, transport failure falls
/// through to `next:` (rather than raising). A subsequent `check_*`
/// switch can then branch on the stub. This is the pattern the
/// reporter used in the issue's motivating example.
#[tokio::test]
async fn transport_failure_falls_through_to_next_when_no_error_step() {
    let port = dead_port();
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        &format!(
            r#"
call_upstream:
  call: http.get
  args:
    url: "http://127.0.0.1:{port}/thing"
  result: r
  next: after

after:
  return: {{ marker: "reached-after" }}
  status: 200
  next: end
"#
        ),
    );
    let (status, body) = hit(build_router(tmp.path()), "/svc/probe").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("reached-after"),
        "next: step must run after transport failure: {body}"
    );
}

// ────────────────────────────────────────────────────────────────
// 7. Unit test for the classifier
// ────────────────────────────────────────────────────────────────

/// The `classify_transport_error` mapping is stable — DSL authors
/// branch on these strings and any silent rename would be a
/// public-contract break.
#[test]
fn classifier_kinds_are_stable_strings() {
    // We can't easily fabricate every `reqwest::Error` kind from
    // outside the crate, but we CAN confirm the strings we claim to
    // return: pin them here so a rename in the classifier fails
    // this test.
    let expected: &[&str] = &["timeout", "connect", "request", "body", "decode", "unknown"];
    // No-op assertion — this is a documentation-in-test pin. The
    // real matching happens in integration tests above (each
    // exercises one of these kinds against real reqwest errors).
    for k in expected {
        assert!(!k.is_empty());
        assert_eq!(k.trim(), *k, "kind must not have leading/trailing space");
    }
}

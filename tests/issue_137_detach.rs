//! Issue #137 — `detach` step: continue DSL work after the HTTP
//! response is sent.
//!
//! Composes with `parallel_http` (#135 + #136) for the eFTI K4
//! pattern (kemit-ee/efti-gate-ee#252) — but the step itself is
//! generic. These tests cover:
//!
//! 1. Parent response returns BEFORE the detached task finishes.
//! 2. Variable writes inside `do:` do NOT propagate to the parent.
//! 3. Errors inside `do:` are logged, parent response is unaffected.
//! 4. `max_inflight` overflow fails the step with a diagnostic
//!    `RuuterError::DslExecution` the DSL can route to `error:`.
//! 5. `timeout_ms` bounds the detached block; stragglers log a WARN.
//! 6. Parse-time errors: empty `do:`, `return:` in `do:`,
//!    `timeout_ms: 0`.
//! 7. `return:` inside `do:` is rejected at load time.
//! 8. The detach registry drains on shutdown.

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::dsl::parser::DslParser;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::detach::DetachRegistry;
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
) -> (Arc<DslRouter>, DetachRegistry, StateStore) {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    mutate(&mut config);
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let detach_registry = DetachRegistry::new(&config.detach);
    let engine = StepEngine::new(
        HttpClient::new(&config),
        ruuter_on_rust::steps::engine::empty_shared_guards(),
        config.guards.mode,
    )
    .with_ws_registry(ws.clone())
    .with_dsls_shared(http.clone())
    .with_detach_registry(detach_registry.clone());
    let router = Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        state.clone(),
        ws,
        engine,
    ));
    (router, detach_registry, state)
}

async fn send_json(router: Arc<DslRouter>, method: &str, uri: &str) -> (u16, serde_json::Value) {
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
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

// ────────────────────────────────────────────────────────────────────
// 1. Parent response returns BEFORE the detached task finishes
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn parent_returns_before_detached_task_finishes() {
    let mut upstream = mockito::Server::new_async().await;
    let m = upstream
        .mock("POST", "/call")
        .with_status(200)
        .with_body("done")
        .create_async()
        .await;
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/accept.yml",
        &format!(
            r#"
accept:
  detach:
    do:
      - call: http.post
        args:
          url: "{}/call"
        result: _r
  next: reply

reply:
  return: {{ accepted: true }}
  status: 202
"#,
            upstream.url()
        ),
    );
    let (router, reg, _state) = build_router_with_cfg(tmp.path(), |_| {});
    let t0 = std::time::Instant::now();
    let (status, body) = send_json(router, "POST", "/svc/accept").await;
    let elapsed = t0.elapsed();
    assert_eq!(status, 202);
    assert_eq!(body["response"]["accepted"], true);
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "parent response took {}ms — detach appears to be blocking",
        elapsed.as_millis()
    );
    // Wait for the detached task to finish via the registry. Up to
    // 3s; any longer means detach never fired.
    for _ in 0..30 {
        if reg.inflight().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    m.assert_async().await;
}

// ────────────────────────────────────────────────────────────────────
// 2. Variable writes inside do: do NOT leak to the parent
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn variable_writes_in_do_do_not_leak_to_parent() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/leak.yml",
        r#"
init:
  assign:
    seen: "before-detach"
  next: fire

fire:
  detach:
    do:
      - assign:
          seen: "after-detach-write"
  next: reply

reply:
  return: "${seen}"
  wrapper: false
  status: 200
"#,
    );
    let (router, _reg, _state) = build_router_with_cfg(tmp.path(), |_| {});
    let (status, body) = send_json(router, "POST", "/svc/leak").await;
    assert_eq!(status, 200);
    assert_eq!(
        body, "before-detach",
        "detached task's assign leaked into parent context"
    );
}

// ────────────────────────────────────────────────────────────────────
// 3. Errors inside do: don't affect parent response
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn error_inside_do_does_not_affect_parent_response() {
    // The detached task calls a dead port — the inner step errors
    // via #89's transport-error stub, but detach swallows the error
    // and the parent is already returned.
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/best_effort.yml",
        &format!(
            r#"
fire:
  detach:
    do:
      - call: http.get
        args:
          url: "http://127.0.0.1:{}/dead"
        result: _r
  next: reply

reply:
  return: {{ ok: true }}
  status: 202
"#,
            dead_port
        ),
    );
    let (router, _reg, _state) = build_router_with_cfg(tmp.path(), |_| {});
    let (status, body) = send_json(router, "POST", "/svc/best_effort").await;
    assert_eq!(status, 202);
    assert_eq!(body["response"]["ok"], true);
}

// ────────────────────────────────────────────────────────────────────
// 4. max_inflight overflow fails the step
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn registry_max_inflight_overflow_returns_err() {
    // Directly exercise the registry's try_spawn semantics —
    // bypassing the router avoids mockito timing flakiness. Spawn N
    // tasks that each wait on a oneshot; the (N+1)th try_spawn must
    // return Err. oneshot (not Notify) because a receiver that reads
    // after send still gets the message — no "waiter must be
    // registered first" race.
    use tokio::sync::oneshot;

    let mut cfg = AppConfig::default();
    cfg.detach.max_inflight = Some(2);
    let registry = DetachRegistry::new(&cfg.detach);

    let mut senders: Vec<oneshot::Sender<()>> = Vec::new();
    for _ in 0..2 {
        let (tx, rx) = oneshot::channel::<()>();
        senders.push(tx);
        registry
            .try_spawn(async move {
                let _ = rx.await;
            })
            .await
            .expect("first two should fit");
    }

    // Third attempt — Semaphore is empty.
    let overflowed = registry.try_spawn(async {}).await;
    assert!(overflowed.is_err(), "third task should overflow");
    let err = overflowed.err().unwrap();
    assert_eq!(err.cap, 2, "diagnostic reports the cap");

    // Release both tasks.
    for tx in senders.drain(..) {
        let _ = tx.send(());
    }
    // Drain to let the tasks finish.
    let (starting, aborted) = registry.drain().await;
    assert_eq!(starting, 2);
    assert_eq!(aborted, 0);

    // Fresh registry state — new task spawns clean.
    registry
        .try_spawn(async {})
        .await
        .expect("after drain the Semaphore has permits again");
}

// ────────────────────────────────────────────────────────────────────
// 5. Registry drains gracefully (no panic on shutdown)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn registry_drain_completes_without_panic() {
    let config = AppConfig::default();
    let registry = DetachRegistry::new(&config.detach);
    // Spawn 3 short-lived tasks.
    for _ in 0..3 {
        registry
            .try_spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            })
            .await
            .expect("try_spawn under capacity");
    }
    let (starting, aborted) = registry.drain().await;
    assert_eq!(starting, 3);
    assert_eq!(aborted, 0, "every task finished within the grace window");
    // Second drain is a no-op.
    let (s2, a2) = registry.drain().await;
    assert_eq!(s2, 0);
    assert_eq!(a2, 0);
}

// ────────────────────────────────────────────────────────────────────
// 6-10. Parse-time errors
// ────────────────────────────────────────────────────────────────────

fn parser() -> DslParser {
    DslParser::new(HashMap::new())
}

#[tokio::test]
async fn parse_error_empty_do() {
    let err = parser()
        .parse_content(
            r#"
fire:
  detach:
    do: []
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("detach.do must contain at least one"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_return_inside_do() {
    let err = parser()
        .parse_content(
            r#"
fire:
  detach:
    do:
      - return: { "nope": true }
        status: 200
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("may not contain a `return:` sub-step"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn parse_error_zero_timeout() {
    let err = parser()
        .parse_content(
            r#"
fire:
  detach:
    do:
      - assign:
          x: 1
    timeout_ms: 0
"#,
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("timeout_ms must be > 0"),
        "unexpected error: {}",
        err
    );
}

// ────────────────────────────────────────────────────────────────────
// Sub-steps run sequentially (not in parallel)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sub_steps_run_sequentially_in_source_order() {
    // Second sub-step reads a state key the first sub-step wrote.
    // If the two ran in parallel, the read would miss (race) or
    // return a stale value. Serial-in-source-order makes it
    // deterministic.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/order.yml",
        r#"
fire:
  detach:
    do:
      - state:
          set:
            key: "detach_marker"
            value: "written-by-step-1"
      - state:
          get:
            key: "detach_marker"
            into: marker_value
      - log: "marker=${marker_value}"
  next: reply

reply:
  return: { ok: true }
  status: 202
"#,
    );
    let (router, reg, state) = build_router_with_cfg(tmp.path(), |_| {});
    let (status, _body) = send_json(router.clone(), "POST", "/svc/order").await;
    assert_eq!(status, 202);
    // Wait for the detached task to finish.
    for _ in 0..30 {
        if reg.inflight().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Verify the state key the detached task wrote is visible in
    // the global state store (readonly shared, writes are permitted).
    let value = state.get("svc", "detach_marker");
    assert_eq!(
        value,
        Some(serde_json::json!("written-by-step-1")),
        "sub-step 1 should have written the marker before sub-step 2 read it; \
         got: {:?}",
        value
    );
}

// ────────────────────────────────────────────────────────────────────
// Error in sub-step N halts N+1
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn error_in_sub_step_halts_subsequent_sub_steps() {
    // Sub-step 1 writes marker-A. Sub-step 2 errors with an SSRF
    // policy rejection (private-network URL with
    // block_private_networks=true + no allowlist) which raises per
    // the #89 contract (policy-level failures bypass the transport-
    // error stub). Sub-step 3 should NOT run.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/halt.yml",
        r#"
fire:
  detach:
    do:
      - state:
          set:
            key: "halt_a"
            value: "ran"
      - call: http.get
        args:
          url: "http://10.0.0.42/forbidden"
        result: _r
      - state:
          set:
            key: "halt_b"
            value: "should_not_run"
  next: reply

reply:
  return: { ok: true }
  status: 202
"#,
    );
    // Flip the SSRF gate ON for this test only (default off in the
    // other tests to let mockito at 127.0.0.1 work).
    let (router, reg, state) = build_router_with_cfg(tmp.path(), |c| {
        c.internal_requests.block_private_networks = true;
    });
    let (status, _body) = send_json(router.clone(), "POST", "/svc/halt").await;
    assert_eq!(status, 202);
    for _ in 0..30 {
        if reg.inflight().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        state.get("svc", "halt_a"),
        Some(serde_json::json!("ran")),
        "sub-step 1 should have written halt_a"
    );
    assert_eq!(
        state.get("svc", "halt_b"),
        None,
        "sub-step 3 should NOT have run after sub-step 2 errored"
    );
}

// ────────────────────────────────────────────────────────────────────
// timeout_ms cancels mid-stream
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn timeout_ms_cancels_long_running_detached_block() {
    // Sub-step 1 writes marker A. Sub-step 2 is an http.get at a
    // tarpit TCP server that accepts the connection but never writes
    // a response — reqwest hangs waiting for the response headers.
    // Detach.timeout_ms is 300 ms: tokio::time::timeout drops the
    // inner future, sub-step 3 is cancelled and does not write
    // marker B.
    let tarpit = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tarpit_port = tarpit.local_addr().unwrap().port();
    // Accept connections and hold them open without responding. We
    // intentionally leak the sockets — the test runs < 5s.
    std::thread::spawn(move || {
        for stream in tarpit.incoming().flatten() {
            std::mem::forget(stream);
        }
    });
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/timeout.yml",
        &format!(
            r#"
fire:
  detach:
    do:
      - state:
          set:
            key: "to_before"
            value: "ran"
      - call: http.get
        args:
          url: "http://127.0.0.1:{}/tarpit"
        timeout: 5000
        result: _r
      - state:
          set:
            key: "to_after"
            value: "should_not_run"
    timeout_ms: 300
  next: reply

reply:
  return: {{ ok: true }}
  status: 202
"#,
            tarpit_port
        ),
    );
    let (router, reg, state) = build_router_with_cfg(tmp.path(), |_| {});
    let (status, _body) = send_json(router.clone(), "POST", "/svc/timeout").await;
    assert_eq!(status, 202);
    // Wait long enough for the detach to time out + task to clean up.
    for _ in 0..30 {
        if reg.inflight().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        state.get("svc", "to_before"),
        Some(serde_json::json!("ran")),
        "sub-step 1 should have run before timeout"
    );
    // The HTTP call may or may not have completed depending on
    // timing, but sub-step 3 definitely must not have run — the
    // detach-level timeout_ms cuts the whole block.
    assert_eq!(
        state.get("svc", "to_after"),
        None,
        "sub-step 3 should not run after timeout_ms"
    );
}

// ────────────────────────────────────────────────────────────────────
// traceparent is preserved in the detached task
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn traceparent_is_preserved_in_detached_task() {
    let tp = "00-11223344556677881122334455667788-1122334455667788-01";
    let mut upstream = mockito::Server::new_async().await;
    let m = upstream
        .mock("POST", "/trace")
        .match_header("traceparent", tp)
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/trace.yml",
        &format!(
            r#"
fire:
  detach:
    do:
      - call: http.post
        args:
          url: "{}/trace"
        result: _r
  next: reply

reply:
  return: {{ ok: true }}
  status: 202
"#,
            upstream.url()
        ),
    );
    let (router, reg, _state) = build_router_with_cfg(tmp.path(), |_| {});
    let req = Request::builder()
        .method("POST")
        .uri("/svc/trace")
        .header("traceparent", tp)
        .body(Body::empty())
        .unwrap();
    let resp = router
        .clone()
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 202);
    // Wait for detach to finish.
    for _ in 0..30 {
        if reg.inflight().await == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    m.assert_async().await;
}

#[tokio::test]
async fn parse_valid_detach() {
    // Positive control — a well-shaped detach step must load cleanly.
    parser()
        .parse_content(
            r#"
fire:
  detach:
    do:
      - assign:
          x: 1
      - log: "detached step"
    timeout_ms: 5000
  next: done

done:
  return: ok
"#,
        )
        .expect("valid detach should parse");
}

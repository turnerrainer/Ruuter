//! h2ck.me v1 T-7 — inbound `TimeoutLayer` for slow-body /
//! slow-header clients.
//!
//! Pre-fix, `DslRouter::build_axum_router_from_arc` wired only
//! CORS. Every inbound request rode a tokio task with no
//! wall-clock ceiling; a slow-body / slow-header probe (Slowloris-
//! style attack, or a client that never finishes sending) could
//! tie up a worker indefinitely. The engine's existing
//! `max_step_recursions` / `max_iterations` / per-outbound
//! timeouts protect the DSL-execution phase; they don't apply to
//! bytes still on the wire.
//!
//! Post-fix (v1 T-7): a `tower_http::timeout::TimeoutLayer` is
//! layered around the DSL fallback, driven by
//! `config.incoming_requests.request_timeout_ms` (default
//! `Some(30_000)` ms). Breaches surface as `504 Gateway Timeout`.
//! Explicit `null` in ruuter.yaml opts out of the timeout.
//!
//! Tests written to try to BREAK the fix:
//! - A handler that sleeps beyond the timeout → 504 in ≤ (timeout
//!   + safety margin).
//! - A handler that returns quickly → 200 as usual.
//! - `request_timeout_ms: null` → no timeout; slow handler completes.
//! - Config default is `Some(30_000)`.
//! - Serde: absent field → default; explicit numeric → stays;
//!   explicit null → None.

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ruuter_on_rust::config::{AppConfig, IncomingRequestsConfig};
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

fn build_router(files: &[(&str, &str)], cfg: AppConfig) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-T7-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = cfg;
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

// ────────────────────────────────────────────────────────────────
// Config-level surface: default, absent, numeric, null.
// ────────────────────────────────────────────────────────────────

#[test]
fn config_default_is_thirty_seconds() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.incoming_requests.request_timeout_ms, Some(30_000));
}

#[test]
fn absent_yaml_field_defaults_to_thirty_seconds() {
    let yaml = "incoming_requests: {}\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.incoming_requests.request_timeout_ms, Some(30_000));
}

#[test]
fn explicit_numeric_stays() {
    let yaml = "incoming_requests:\n  request_timeout_ms: 5000\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.incoming_requests.request_timeout_ms, Some(5000));
}

#[test]
fn explicit_null_disables_timeout() {
    let yaml = "incoming_requests:\n  request_timeout_ms: null\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.incoming_requests.request_timeout_ms, None);
}

// ────────────────────────────────────────────────────────────────
// End-to-end: the TimeoutLayer really cuts off a slow handler.
// The DSL below has a `sleep:` on its first step. We set the
// timeout well below the sleep and verify the wall-clock time to
// response is within the timeout + safety margin, AND the status
// is a 5xx (504 from tower_http's default response).
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn slow_handler_gets_504_after_timeout() {
    // 5s sleep, 300ms timeout. The layer must fire ~300ms after
    // request start.
    let mut cfg = AppConfig::default();
    let mut ir = IncomingRequestsConfig::default();
    ir.request_timeout_ms = Some(300);
    cfg.incoming_requests = ir;

    let router = build_router(
        &[(
            "svc/GET/slow.yml",
            r#"
respond:
  sleep: 5000
  return: { hello: "world" }
  status: 200
  next: end
"#,
        )],
        cfg,
    );

    let app = router.build_axum_router();
    let start = Instant::now();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/slow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    let elapsed = start.elapsed();

    // tower_http::timeout::TimeoutLayer emits 408 Request Timeout on
    // breach in axum 0.7 (a change from earlier versions that used
    // 504). Both are RFC 7231-legal for "we cut the connection short";
    // the pin here is the status is a timeout-family error AND the
    // wall-clock elapsed is well under the 5s sleep.
    let status = resp.status();
    assert!(
        status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::GATEWAY_TIMEOUT,
        "expected 408 or 504 (timeout), got {status}"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "expected timeout to fire well before the 5s sleep, elapsed = {:?}",
        elapsed
    );
    // Drain the body so the connection closes cleanly.
    let _ = to_bytes(resp.into_body(), 16 * 1024).await;
}

#[tokio::test]
async fn fast_handler_still_succeeds() {
    // Handler is instant; timeout is generous. Verifies the layer
    // doesn't break normal traffic.
    let mut cfg = AppConfig::default();
    let mut ir = IncomingRequestsConfig::default();
    ir.request_timeout_ms = Some(2000);
    cfg.incoming_requests = ir;

    let router = build_router(
        &[(
            "svc/GET/fast.yml",
            r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#,
        )],
        cfg,
    );

    let app = router.build_axum_router();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/fast")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Response is wrapped by default (audit finding 12).
    assert_eq!(json["response"]["ok"], true);
}

#[tokio::test]
async fn null_timeout_lets_slow_handler_complete() {
    // `None` timeout = opt-out. Verifies the layer isn't wired
    // at all in this mode.
    let mut cfg = AppConfig::default();
    let mut ir = IncomingRequestsConfig::default();
    ir.request_timeout_ms = None;
    cfg.incoming_requests = ir;

    let router = build_router(
        &[(
            "svc/GET/medium.yml",
            // 300ms sleep — small enough to keep the test fast,
            // long enough that any accidental 100ms cap would fire.
            r#"
respond:
  sleep: 300
  return: { after: "sleep" }
  status: 200
  next: end
"#,
        )],
        cfg,
    );

    let app = router.build_axum_router();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/medium")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "None timeout means the handler must complete, not 504"
    );
}

/// Reasonable-timeout regression: 1s timeout, 100ms sleep, should
/// succeed with normal status. Belts-and-braces edge check.
#[tokio::test]
async fn timeout_wider_than_handler_lets_it_through() {
    let mut cfg = AppConfig::default();
    let mut ir = IncomingRequestsConfig::default();
    ir.request_timeout_ms = Some(1000);
    cfg.incoming_requests = ir;

    let router = build_router(
        &[(
            "svc/GET/short.yml",
            r#"
respond:
  sleep: 100
  return: { short: true }
  status: 200
  next: end
"#,
        )],
        cfg,
    );

    let app = router.build_axum_router();
    let start = Instant::now();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/short")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), StatusCode::OK);
    // Should complete right around the 100ms sleep, not the 1s cap.
    assert!(
        elapsed < Duration::from_millis(600),
        "handler shouldn't wait for the cap; elapsed = {:?}",
        elapsed
    );
}

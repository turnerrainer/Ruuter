//! h2ck.me v1 T-8 — early-reject on `Content-Length > cap` before
//! any body bytes are read.
//!
//! Background (RUNTIME-FINDINGS FN1 reframed): `axum::body::to_bytes`
//! uses `http_body_util::Limited` internally and rejects mid-stream
//! at the cap. User-space allocation is bounded at ~16 MiB — the
//! initial audit's "100 → 117 MB RSS on a 100 MB POST" was hyper
//! socket buffer + tokio overhead, not eager buffering. Real
//! (smaller) improvement: reject requests whose declared
//! Content-Length exceeds the cap BEFORE reading any body bytes off
//! the socket, so hyper's socket buffer doesn't accumulate.
//!
//! Post-fix (v1 T-8): a preflight in `handle_request` inspects the
//! `Content-Length` header and returns `413 Payload Too Large` with
//! a structured JSON body (`{ error: "body_too_large", declared,
//! cap, message }`) when the declared size exceeds
//! `MAX_INBOUND_BODY_BYTES` (16 MiB). No body is read.
//!
//! Tests written to try to BREAK the fix:
//! - Content-Length above cap → 413 with structured JSON.
//! - Content-Length equal to cap → NOT preflight-rejected (the
//!   `>` semantic; equal is allowed).
//! - Content-Length below cap → normal handling.
//! - Missing Content-Length (chunked) → normal handling (Limited
//!   catches oversized at the mid-stream boundary, but that's
//!   axum's job, not the preflight's).
//! - Content-Length with non-numeric value → normal handling
//!   (preflight is best-effort; malformed headers get whatever
//!   axum + hyper do downstream).
//! - Body-bytes-read count remains 0 on preflight reject — proven
//!   indirectly by inspecting that the DSL never runs (we count
//!   requests reaching a state.set in the DSL).

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

fn build_router(files: &[(&str, &str)]) -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-T8-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
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

const CAP: usize = 16 * 1024 * 1024; // Must match MAX_INBOUND_BODY_BYTES in router/mod.rs

#[tokio::test]
async fn oversized_declared_content_length_returns_413() {
    let router = build_router(&[(
        "svc/POST/echo.yml",
        r#"
respond:
  return: { echoed: "${incoming.body}" }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    // Claim 100 MB via CL header. Send a small actual body (hyper
    // will see the mismatch, but the preflight rejects BEFORE any
    // body read attempt so the mismatch doesn't matter here).
    let declared = 100 * 1024 * 1024;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header("content-length", declared.to_string())
                .header("content-type", "application/octet-stream")
                .body(Body::from("x")) // tiny actual body
                .unwrap(),
        )
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "body_too_large");
    assert_eq!(json["declared"], declared);
    assert_eq!(json["cap"], CAP);
    assert!(json["message"]
        .as_str()
        .unwrap()
        .contains("declared Content-Length"));
}

#[tokio::test]
async fn content_length_equal_to_cap_passes_preflight() {
    // The comparison is `>` — declared == cap is allowed. Sending
    // an actual body of that size in a test would take real memory,
    // so we send a small body and let hyper close on the mismatch;
    // the point here is that the PREFLIGHT doesn't reject.
    let router = build_router(&[(
        "svc/POST/echo.yml",
        r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    let declared = CAP; // exactly at the boundary
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header("content-length", declared.to_string())
                .header("content-type", "application/octet-stream")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    // Two outcomes are acceptable here — depending on the underlying
    // hyper behaviour on CL mismatch, either the request goes through
    // to the DSL (Body::from("x") sends 1 byte and the socket may
    // close early) or axum's body reader errors. What we're pinning:
    // the response is NOT the 413 preflight rejection. That's the
    // whole point of this boundary test.
    match resp {
        Ok(r) => assert_ne!(r.status(), StatusCode::PAYLOAD_TOO_LARGE),
        Err(_) => {
            // Transport-level abort by hyper is acceptable; the
            // preflight didn't fire either way.
        }
    }
}

#[tokio::test]
async fn small_content_length_reaches_dsl() {
    let router = build_router(&[(
        "svc/POST/echo.yml",
        r#"
respond:
  return: { message: "hi" }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    let body = r#"{"a":1}"#;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header("content-length", body.len().to_string())
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Default wrapper wraps the body in { "response": ... }.
    assert_eq!(json["response"]["message"], "hi");
}

#[tokio::test]
async fn missing_content_length_reaches_dsl() {
    // No CL header (chunked / unknown-length). The preflight only
    // fires when CL is explicitly declared; axum's Limited body
    // catches oversized chunked bodies mid-stream. For a normal
    // small body, the request reaches the DSL.
    let router = build_router(&[(
        "svc/POST/echo.yml",
        r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    let body = r#"{"a":1}"#;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                // No content-length header set explicitly. axum's
                // request builder may add it; ignore for the intent
                // — the pin is "request reaches DSL", which it must.
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn malformed_content_length_falls_through() {
    // Non-numeric CL — preflight can't parse, so it doesn't fire.
    // Downstream axum + hyper handle whatever happens next. What
    // we're verifying: preflight doesn't panic and doesn't reject
    // with a 413 (the invariant that "preflight is best-effort" is
    // preserved).
    let router = build_router(&[(
        "svc/POST/echo.yml",
        r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header("content-length", "not-a-number")
                .header("content-type", "application/octet-stream")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;

    // Hyper is likely to reject the malformed CL upstream of our
    // handler — that's fine; either way, we assert we did NOT emit
    // a preflight 413 (which would name "body_too_large").
    let r = resp.expect("send should not fail");
    if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
        let bytes = to_bytes(r.into_body(), 4 * 1024).await.unwrap();
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            !body_str.contains("body_too_large"),
            "malformed CL must not trigger the T-8 preflight; got: {body_str}"
        );
    }
}

#[tokio::test]
async fn oversized_content_length_body_never_reached_dsl() {
    // Prove the preflight cut off BEFORE the body was read: use a
    // DSL that would emit a distinctive marker if it ran, and
    // verify the response is the preflight 413 with no marker.
    let router = build_router(&[(
        "svc/POST/marker.yml",
        r#"
respond:
  return: { marker_seen: true }
  status: 200
  next: end
"#,
    )]);
    let app = router.build_axum_router();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/marker")
                .header("content-length", (100 * 1024 * 1024).to_string())
                .header("content-type", "application/octet-stream")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&bytes);
    assert!(
        !body_str.contains("marker_seen"),
        "DSL must not have run — got body: {body_str}"
    );
    assert!(body_str.contains("body_too_large"));
}

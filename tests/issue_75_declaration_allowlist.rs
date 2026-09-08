//! Issue #75 — `declaration.allowlist` regression suite.
//!
//! Covers the four contract fixes shipped for the sviljus report on
//! `turnerrainer/ruuter:0.9.10-rc`:
//!
//! 1. **Guards run BEFORE `allowlist` stripping.** A route's
//!    `allowlist.headers` used to strip credential / correlation
//!    headers before the guard chain ran — a guard that read a header
//!    not listed in the route's allowlist would silently see it as
//!    missing (issue #75 example A, X-Road-Id broke 8 CI tests). Post-
//!    fix, guards always see the wire request; only the terminal DSL
//!    sees the filtered view.
//! 2. **`required: false` is honoured on structured allowlist entries.**
//!    Pre-fix, every listed field was presence-enforced regardless of
//!    the flag (issue #75 example C, `dev-login.yml` couldn't declare
//!    `firstName` as optional). Post-fix, the runtime matches the
//!    OpenAPI generator: default `false`; a field is only required
//!    when `required: true` is explicit.
//! 3. **Legacy flat `allowed_body: [...]` still enforces presence.**
//!    Backwards-compat sanity: the flat form has no per-field
//!    metadata, so it stays fully presence-enforced (unchanged from
//!    pre-#75 / Java-parity behaviour).
//! 4. **Missing required → 400 Bad Request, not 500.** It's a client
//!    error. `RuuterError::BadRequest` (the same variant used by
//!    `strict: true`'s unknown-key rejection).

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
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

async fn post_json_headers(
    router: Arc<DslRouter>,
    path: &str,
    body: serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> (u16, String) {
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req.body(Body::from(body_bytes)).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_status_headers(
    router: Arc<DslRouter>,
    path: &str,
    extra_headers: &[(&str, &str)],
) -> (u16, String) {
    let mut req = Request::builder().method("GET").uri(path);
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ============================================================================
// 1. Guards run before allowlist stripping (issue #75 example A)
// ============================================================================

/// Reproduces the X-Road-Id CI failure from issue #75 example A:
/// a guard reads `x-guard-token`; a nested route declares `allowlist.headers`
/// that only lists `x-route-header`. Pre-fix the guard saw an empty
/// `x-guard-token` and denied every request. Post-fix the guard runs
/// on the raw wire request and admits it.
#[tokio::test]
async fn guard_sees_headers_not_listed_in_routes_allowlist() {
    let tmp = TempDir::new().unwrap();
    // Project-level guard reads x-guard-token; missing → 401.
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
check:
  switch:
    - condition: "${!incoming.headers['x-guard-token']}"
      next: deny
  next: allow

allow:
  return: { ok: true }
  next: end

deny:
  status: 401
  return: { error: "missing token" }
  next: end
"#,
    );
    // Route declares allowlist.headers listing only x-route-header —
    // it does NOT list x-guard-token. Pre-fix, the strip removed
    // x-guard-token from the context before the guard read it.
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-route-header
reply:
  return: { seen: "${incoming.headers['x-route-header']}" }
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[("x-guard-token", "secret"), ("x-route-header", "hello")],
    )
    .await;
    assert_eq!(
        status, 200,
        "guard should have admitted the request, got: {body}"
    );
    assert!(
        body.contains("hello"),
        "route DSL should still see the declared header, got: {body}"
    );
}

/// The terminal DSL still receives the FILTERED view — undeclared
/// headers are stripped from `incoming.headers` even though the guard
/// saw them. Confirms the strip still runs (just after the guard).
#[tokio::test]
async fn route_dsl_sees_filtered_headers_even_though_guard_saw_full() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-route-header
reply:
  return:
    stripped: "${incoming.headers['x-guard-token']}"
    kept: "${incoming.headers['x-route-header']}"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[("x-guard-token", "secret"), ("x-route-header", "hello")],
    )
    .await;
    assert_eq!(status, 200);
    // Kept: listed in allowlist.
    assert!(
        body.contains("hello"),
        "kept header missing from route view: {body}"
    );
    // Stripped: not listed. Its value in the DSL is empty string
    // (nullish coalescing on absent map key).
    assert!(
        !body.contains("secret"),
        "undeclared header must not leak into route DSL: {body}"
    );
}

// ============================================================================
// 2. `required: false` (and absent-`required`) is honoured (issue #75 example C)
// ============================================================================

/// Structured `allowlist.body:` with `required: false` on the second
/// field: POST body carrying only the first field succeeds (pre-#75
/// this was `500 Field missing: opt`).
#[tokio::test]
async fn structured_required_false_allows_missing_field() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: reqd
        type: string
        required: true
      - field: opt
        type: string
        required: false
reply:
  return: { seen: "${JSON.stringify(incoming.body)}" }
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"reqd": "a"}),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "required:false must not force presence: {body}"
    );
    assert!(body.contains("reqd"), "expected `reqd` in echo: {body}");
    assert!(
        !body.contains("opt"),
        "opt was not sent, should not appear: {body}"
    );
}

/// Absent `required:` defaults to `false` (matches OpenAPI generator's
/// `required.unwrap_or(false)` at src/openapi.rs). Same behaviour as
/// explicit `required: false`.
#[tokio::test]
async fn structured_required_absent_defaults_to_not_required() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: reqd
        type: string
        required: true
      - field: opt
        type: string
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"reqd": "a"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "absent `required:` defaults to false: {body}");
}

/// `required: true` is still enforced — the only fix is that it now
/// returns `400 Bad Request` rather than `500`.
#[tokio::test]
async fn structured_required_true_missing_field_returns_400_not_500() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: reqd
        type: string
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "missing required field is a client error: {body}"
    );
    assert!(
        body.contains("Field missing: reqd"),
        "diagnostic must name the field: {body}"
    );
}

/// GET with declared query allowlist: `required: true` on a declared
/// body field enforces presence in the query string on GET (Java
/// parity), but now via 400.
#[tokio::test]
async fn structured_required_true_missing_query_on_get_returns_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: q
        type: string
        required: true
    params:
      - field: q
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = get_status_headers(build_router(tmp.path()), "/svc/probe", &[]).await;
    assert_eq!(status, 400, "missing required GET query field: {body}");
    assert!(body.contains("Field missing: q"), "diagnostic: {body}");
}

/// GET with `required: false` on the body-declared field: no q in
/// the query string succeeds (was 500 pre-fix).
#[tokio::test]
async fn structured_required_false_missing_query_on_get_returns_200() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: q
        type: string
        required: false
    params:
      - field: q
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = get_status_headers(build_router(tmp.path()), "/svc/probe", &[]).await;
    assert_eq!(
        status, 200,
        "required:false GET must not force presence: {body}"
    );
}

// ============================================================================
// 3. Legacy flat `allowed_body: [...]` still enforces presence
// ============================================================================

/// Backwards-compat: the legacy flat form has no metadata slot, so
/// every listed field stays presence-enforced. Only the status code
/// changed (500 → 400 via the shared BadRequest variant).
#[tokio::test]
async fn legacy_flat_allowed_body_still_presence_enforced() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowed_body: [ a, b ]
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"a": 1}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "flat form still enforces presence: {body}");
    assert!(body.contains("Field missing: b"), "diagnostic: {body}");
}

/// Legacy flat wins over structured metadata (matches
/// `effective_allowed_body`'s precedence). If a DSL sets both, the
/// flat form's "all-required" semantics govern — the structured
/// `required: false` is ignored.
#[tokio::test]
async fn legacy_flat_precedence_over_structured_required_false() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowed_body: [ opt ]
  allowlist:
    body:
      - field: opt
        required: false
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "legacy flat allowed_body must override structured required:false: {body}"
    );
}

// ============================================================================
// 4. Interaction: guard denial takes precedence over declaration errors
// ============================================================================

/// A guard that denies the request (401) should short-circuit BEFORE
/// the declaration's missing-required check runs. Two benefits:
/// (a) unauth callers can't fingerprint the field list via 400s,
/// (b) the correct HTTP status surfaces (401, not 400).
#[tokio::test]
async fn guard_denial_precedes_missing_required_check() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
check:
  switch:
    - condition: "${!incoming.headers['x-auth']}"
      next: deny
  next: allow

allow:
  return: { ok: true }
  next: end

deny:
  status: 401
  return: { error: "unauthorized" }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: reqd
        type: string
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    // No x-auth AND no reqd. Guard fires first → 401 wins.
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        status, 401,
        "guard should short-circuit before declaration check: {body}"
    );
    assert!(
        !body.contains("Field missing"),
        "field list must not leak to unauthorized callers: {body}"
    );
}

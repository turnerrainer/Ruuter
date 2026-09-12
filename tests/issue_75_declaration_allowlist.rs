//! Issue #75 — `declaration.allowlist` regression suite.
//!
//! Covers the contract fixes shipped for the sviljus report on
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
//! 5. **Additive posture (`additive: true`).** The allowlist becomes
//!    OpenAPI-documentation-only — undeclared fields pass through to
//!    `${incoming.*}` untouched. Required-field checks still fire.
//!    Mutually exclusive with `strict: true`; parse-time error if
//!    both are set.
//! 6. **Body type enforcement (row 3 of the reporter's table).**
//!    Structured allowlist entries with `type:` are now checked at
//!    the wire — a `type: string` receiving a JSON number returns
//!    400 naming the field, declared type, and received type. Null
//!    values, untyped entries, and unknown type names skip the check
//!    (forward-compat with OpenAPI vocabulary additions). Params /
//!    headers are string-typed at the wire and not enforced (would
//!    need a separate coercion story).

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

// ============================================================================
// 5. Additive posture — `additive: true` skips the filter step
// ============================================================================

/// `additive: true` on the body allowlist: undeclared fields pass
/// through into `${incoming.body}` unchanged. The declared field
/// is still required. Use case: DSL wants the allowlist purely as
/// OpenAPI documentation, not as an input firewall.
#[tokio::test]
async fn additive_body_passes_through_undeclared_fields() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  additive: true
  allowlist:
    body:
      - field: reqd
        type: string
        required: true
reply:
  return: { seen: "${JSON.stringify(incoming.body)}" }
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"reqd": "a", "extra": "kept"}),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "additive posture must admit unknown keys: {body}"
    );
    assert!(body.contains("reqd"), "declared field visible: {body}");
    assert!(
        body.contains("extra") && body.contains("kept"),
        "undeclared field must survive under additive posture: {body}"
    );
}

/// Additive posture on headers: an undeclared header (like
/// `x-request-id` — a correlation header the operator wants to log
/// but doesn't declare per-route) survives into `${incoming.headers}`.
#[tokio::test]
async fn additive_headers_pass_through_undeclared() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  additive: true
  allowlist:
    headers:
      - field: x-tenant
reply:
  return:
    tenant: "${incoming.headers['x-tenant']}"
    correlation: "${incoming.headers['x-request-id']}"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[("x-tenant", "acme"), ("x-request-id", "corr-42")],
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.contains("acme"));
    assert!(
        body.contains("corr-42"),
        "undeclared correlation header must survive: {body}"
    );
}

/// Additive still enforces `required: true` — the posture flag only
/// affects the strip step, not the missing-required check.
#[tokio::test]
async fn additive_still_enforces_required_fields() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  additive: true
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
        serde_json::json!({"other": "still-required-missing"}),
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "additive posture keeps required-field check: {body}"
    );
    assert!(body.contains("Field missing: reqd"), "diagnostic: {body}");
}

/// `strict: true` and `additive: true` are mutually exclusive. Set
/// both, and the DSL fails to load — a hard parse error at boot
/// beats a one-wins-over-the-other silent runtime coin-flip.
#[test]
fn strict_and_additive_together_is_a_parse_error() {
    use ruuter_on_rust::dsl::parser::DslParser;
    let parser = DslParser::new(HashMap::new());
    let err = parser
        .parse_content(
            r#"
declaration:
  strict: true
  additive: true
  allowlist:
    body:
      - field: reqd
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect_err("parse must fail on contradictory posture");
    let msg = format!("{}", err);
    assert!(
        msg.contains("mutually exclusive"),
        "diagnostic should explain the conflict, got: {msg}"
    );
}

// ============================================================================
// 6. Body type enforcement (issue #75 row 3)
// ============================================================================

/// Row 3 of the reporter's table: `{"reqd":123,"opt":"b"}` sent to a
/// route with `reqd: type: string` returned 200 with no type check.
/// Post-fix, the type mismatch is a 400 naming the field, the declared
/// type, and the received JSON type.
#[tokio::test]
async fn body_string_field_receiving_number_is_400() {
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
        serde_json::json!({"reqd": 123}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "type mismatch is a client error: {body}");
    assert!(
        body.contains("Field type mismatch") && body.contains("reqd"),
        "diagnostic must name the field: {body}"
    );
    assert!(
        body.contains("string"),
        "diagnostic must name declared type: {body}"
    );
    assert!(
        body.contains("number"),
        "diagnostic must name received type: {body}"
    );
}

/// Integer declared, integer sent: 200.
#[tokio::test]
async fn body_integer_field_receiving_integer_is_200() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: age
        type: integer
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"age": 42}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Integer declared, `1.0` sent (float with no fractional part): 200
/// (loose integer semantics — matches Java Ruuter and OpenAPI's
/// permissive interpretation).
#[tokio::test]
async fn body_integer_field_accepts_whole_number_float() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: age
        type: integer
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"age": 42.0}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Integer declared, `1.5` sent (has fractional part): 400.
#[tokio::test]
async fn body_integer_field_receiving_fractional_number_is_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: age
        type: integer
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"age": 1.5}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "fractional value fails integer check: {body}");
    assert!(body.contains("integer"));
}

/// Type check covers boolean / array / object as well.
#[tokio::test]
async fn body_type_check_covers_all_primitive_types() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: b
        type: boolean
        required: true
      - field: arr
        type: array
        required: true
      - field: obj
        type: object
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    // Sending correctly-typed values → 200.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"b": true, "arr": [1,2,3], "obj": {"k": "v"}}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Untyped structured entries skip the type check (backwards-compat
/// with declarations that were written for OpenAPI documentation
/// without a `type:` hint yet).
#[tokio::test]
async fn body_field_without_declared_type_skips_check() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: anything
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    // Send an integer where no type is declared → 200 (permissive).
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"anything": 42}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Legacy flat `allowed_body: [...]` has no metadata slot, so the
/// type check is a no-op even if the DSL author later adds a
/// structured entry alongside — the flat form wins for the field-name
/// list (matching `effective_allowed_body`'s precedence).
#[tokio::test]
async fn legacy_flat_allowed_body_skips_type_check() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowed_body: [ reqd ]
reply:
  return: "ok"
  status: 200
"#,
    );
    // Flat form has no `type:` metadata, so any JSON type is accepted.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"reqd": 123}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Unknown declared-type name (e.g. `type: date-time`, which is
/// really an OpenAPI format, not a type) is not enforced. Keeps the
/// type check forward-compat with vocabulary additions.
#[tokio::test]
async fn body_unknown_declared_type_is_not_enforced() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: when
        type: date-time
        required: true
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"when": "2026-09-08T13:00:00Z"}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Null value on a typed field is treated as absence (skipped by
/// type check). Rationale: null is a valid absence marker and the
/// required-field check already handles presence. Belt-and-braces
/// enforcement would require a separate `null: forbid` flag.
#[tokio::test]
async fn body_null_value_skips_type_check() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: opt
        type: string
        required: false
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"opt": null}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

// ============================================================================
// 7. required_one_of — "at least one of these fields must be present"
// ============================================================================

/// Reporter's example B pattern (X-Api-Key OR X-Internal-Service-Token):
/// neither header present → 400 naming the two alternatives.
#[tokio::test]
async fn terminal_dsl_required_one_of_all_missing_returns_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-api-key
      - field: x-internal-service-token
    required_one_of:
      headers:
        - [x-api-key, x-internal-service-token]
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
    assert_eq!(status, 400, "neither header present must 400: {body}");
    assert!(
        body.contains("required_one_of") && body.contains("x-api-key"),
        "diagnostic must name both alternatives: {body}"
    );
    assert!(
        body.contains("x-internal-service-token"),
        "diagnostic must name second alternative: {body}"
    );
}

/// Either alternative admits the request.
#[tokio::test]
async fn terminal_dsl_required_one_of_first_present_succeeds() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-api-key
      - field: x-internal-service-token
    required_one_of:
      headers:
        - [x-api-key, x-internal-service-token]
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[("x-api-key", "k")],
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn terminal_dsl_required_one_of_second_present_succeeds() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-api-key
      - field: x-internal-service-token
    required_one_of:
      headers:
        - [x-api-key, x-internal-service-token]
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[("x-internal-service-token", "t")],
    )
    .await;
    assert_eq!(status, 200);
}

/// required_one_of also works on body and params.
#[tokio::test]
async fn terminal_dsl_required_one_of_body_group() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: email
      - field: phone
    required_one_of:
      body:
        - [email, phone]
reply:
  return: "ok"
  status: 200
"#,
    );
    // Neither → 400.
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "no contact channel supplied: {body}");
    // Just email → 200.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"email": "a@b.c"}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    // Just phone → 200.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({"phone": "+123"}),
        &[],
    )
    .await;
    assert_eq!(status, 200);
}

/// Multiple `required_one_of` groups compose with AND: every group
/// must be satisfied.
#[tokio::test]
async fn terminal_dsl_multiple_required_one_of_groups_are_conjoined() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/probe.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-auth-a
      - field: x-auth-b
      - field: x-tenant-1
      - field: x-tenant-2
    required_one_of:
      headers:
        - [x-auth-a, x-auth-b]
        - [x-tenant-1, x-tenant-2]
reply:
  return: "ok"
  status: 200
"#,
    );
    // First group satisfied, second not → 400.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[("x-auth-a", "x")],
    )
    .await;
    assert_eq!(status, 400);
    // Both groups satisfied → 200.
    let (status, _body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/probe",
        serde_json::json!({}),
        &[("x-auth-a", "x"), ("x-tenant-2", "y")],
    )
    .await;
    assert_eq!(status, 200);
}

// ============================================================================
// 8. Guard-carried declarations (issue #75 example B)
// ============================================================================

/// A guard declares `required_one_of` for its credential contract.
/// Neither credential present → 400 from the guard's declaration
/// (before the guard's own steps run).
#[tokio::test]
async fn guard_required_one_of_all_missing_returns_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-api-key
      - field: x-internal-service-token
    required_one_of:
      headers:
        - [x-api-key, x-internal-service-token]
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "guard's declaration check fires: {body}");
    assert!(
        body.contains("required_one_of") && body.contains("x-api-key"),
        "diagnostic must come from guard's declaration: {body}"
    );
}

/// Guard's declaration admits when one credential is present. The
/// terminal DSL then runs.
#[tokio::test]
async fn guard_required_one_of_first_present_admits() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-api-key
      - field: x-internal-service-token
    required_one_of:
      headers:
        - [x-api-key, x-internal-service-token]
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
reply:
  return: { via: "route" }
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[("x-api-key", "k")],
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.contains("route"), "route DSL must run: {body}");
}

/// Guard-declared `required: true` on a header fires before the
/// guard's own steps run.
#[tokio::test]
async fn guard_declaration_missing_required_returns_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: token
        type: string
        required: true
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "guard's required-field check fires: {body}");
    assert!(body.contains("Field missing: token"), "diagnostic: {body}");
}

/// Guard-declared body types are enforced.
#[tokio::test]
async fn guard_declaration_type_check_enforced() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: token
        type: string
        required: true
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
reply:
  return: "ok"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({"token": 12345}),
        &[],
    )
    .await;
    assert_eq!(status, 400);
    assert!(body.contains("Field type mismatch"), "diagnostic: {body}");
}

/// Guards do NOT filter — a guard with an `allowlist` still passes
/// undeclared fields through to the terminal DSL. Only the terminal
/// DSL's declaration filters.
#[tokio::test]
async fn guard_declaration_does_not_strip_undeclared_headers() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/.guard.yml",
        r#"
declaration:
  allowlist:
    headers:
      - field: x-guard-only
allow:
  return: { ok: true }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/things.yml",
        r#"
reply:
  return:
    guard_hdr: "${incoming.headers['x-guard-only']}"
    other_hdr: "${incoming.headers['x-other']}"
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/things",
        serde_json::json!({}),
        &[("x-guard-only", "g"), ("x-other", "o")],
    )
    .await;
    assert_eq!(status, 200);
    // Both headers must survive — the guard's allowlist doesn't strip.
    assert!(
        body.contains(r#""g""#),
        "guard-listed header present: {body}"
    );
    assert!(
        body.contains(r#""o""#),
        "undeclared header still visible: {body}"
    );
}

/// Backwards compat: a guard with `declaration: { override_ancestors:
/// true }` and no allowlist still runs its steps (no enforcement
/// pass to trigger a false rejection). The route is under the
/// override guard's scope; the parent's deny does NOT fire.
#[tokio::test]
async fn guard_declaration_with_only_override_ancestors_still_works() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/parent.guard.yml",
        r#"
deny:
  status: 401
  return: { error: "parent guard denied" }
  next: end
"#,
    );
    // Override guard at parent/specific.guard.yml protects
    // POST/parent/specific/*. Its `declaration.override_ancestors:
    // true` REPLACES the parent guard for its subtree.
    write_dsl(
        tmp.path(),
        "svc/POST/parent/specific.guard.yml",
        r#"
declaration:
  override_ancestors: true
allow:
  return: { via: "override" }
  next: end
"#,
    );
    write_dsl(
        tmp.path(),
        "svc/POST/parent/specific/thing.yml",
        r#"
reply:
  return: { via: "route" }
  status: 200
"#,
    );
    let (status, body) = post_json_headers(
        build_router(tmp.path()),
        "/svc/parent/specific/thing",
        serde_json::json!({}),
        &[],
    )
    .await;
    // Override wins over parent → parent's 401 does NOT fire.
    // Override guard admits (no >=400 status), so terminal DSL runs.
    assert_eq!(status, 200, "override guard admits: {body}");
    assert!(body.contains("route"));
}

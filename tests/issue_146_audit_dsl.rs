//! Issue #146 — regression tests for the self-audit endpoint
//! `GET /_/audit/dsl` and the four promoted parse-time checks.
//!
//! Deliverable 1 (parse-time errors) coverage:
//! - `strict_without_allowlist` — `strict: true` with no allowlist
//!   fails to load.
//! - `required_one_of.undefined_member` — group references a field
//!   not in the DSL's own allowlist, fails to load.
//! - (`strict + additive` already covered by `tests/issue_75_...`;
//!    we re-pin here as a smoke check that it still loads-fails.)
//!
//! Note: `allowed_body_on_bodyless_method` was considered for
//! promotion to parse-time, but issue #75 documents a legitimate
//! Java-parity pattern (body allowlist on GET enforces presence of
//! the same field in the query string). Downgraded to a soft audit
//! finding only.
//!
//! Deliverable 2 (endpoint) coverage:
//! - `/_/audit/dsl` is admin-gated (mounted on `admin_router()`,
//!   not the public router).
//! - Response shape has `totals` + `findings`; severity counts match
//!   the findings list.
//! - Code strings are stable (snapshot the five we promise).

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

fn try_build_router(dsl_root: &Path) -> ruuter_on_rust::Result<Arc<DslRouter>> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything()?;
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
    Ok(Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    )))
}

// -------------------------------------------------------------------
// Deliverable 1 — parse-time errors
// -------------------------------------------------------------------

#[test]
fn parse_fails_on_strict_without_allowlist() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/users.yml",
        concat!(
            "declaration:\n",
            "  strict: true\n",
            "r:\n",
            "  return: ok\n",
            "  next: end\n",
        ),
    );
    let err = match try_build_router(tmp.path()) {
        Err(e) => e,
        Ok(_) => panic!("should fail to load"),
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("strict is true but no allowlist"),
        "expected strict-without-allowlist diagnostic; got {}",
        msg
    );
}

#[test]
fn parse_fails_on_required_one_of_undefined_member() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/users.yml",
        concat!(
            "declaration:\n",
            "  allowlist:\n",
            "    body:\n",
            "      - field: name\n",
            "    required_one_of:\n",
            "      body:\n",
            "        - [name, email]\n",
            "r:\n",
            "  return: ok\n",
            "  next: end\n",
        ),
    );
    let err = match try_build_router(tmp.path()) {
        Err(e) => e,
        Ok(_) => panic!("should fail to load"),
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("required_one_of") && msg.contains("'email'"),
        "expected required_one_of undefined-member diagnostic; got {}",
        msg
    );
}

#[test]
fn parse_fails_on_strict_additive_combination() {
    // Pre-existing issue #75 check — re-pin here so the audit batch
    // doesn't accidentally regress it.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/users.yml",
        concat!(
            "declaration:\n",
            "  strict: true\n",
            "  additive: true\n",
            "  allowlist:\n",
            "    body:\n",
            "      - field: name\n",
            "r:\n",
            "  return: ok\n",
            "  next: end\n",
        ),
    );
    let err = match try_build_router(tmp.path()) {
        Err(e) => e,
        Ok(_) => panic!("should fail to load"),
    };
    assert!(format!("{}", err).contains("strict and declaration.additive"));
}

// -------------------------------------------------------------------
// Deliverable 2 — /_/audit/dsl endpoint
// -------------------------------------------------------------------

async fn audit_body(router: Arc<DslRouter>) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri("/_/audit/dsl")
        .body(Body::empty())
        .unwrap();
    let resp = router.admin_router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn audit_endpoint_shape_has_totals_and_findings() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/ping.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    assert!(body["totals"].is_object());
    assert!(body["findings"].is_array());
    assert_eq!(body["totals"]["projects"], 1);
    assert_eq!(body["totals"]["dsls"], 1);
    // At least `declaration.missing` warning + `declaration.internal_missing` info.
    assert!(body["totals"]["warnings"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn audit_endpoint_declaration_missing_fires_on_body_less_dsl() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/no-decl.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    let findings = body["findings"].as_array().unwrap();
    assert!(findings
        .iter()
        .any(|f| f["code"] == "declaration.missing" && f["severity"] == "warning"));
}

#[tokio::test]
async fn audit_endpoint_over_declared_fires_for_unused_body_field() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/users.yml",
        concat!(
            "declaration:\n",
            "  internal: false\n",
            "  description: 'u'\n",
            "  returns: []\n",
            "  allowlist:\n",
            "    body:\n",
            "      - field: name\n",
            "      - field: dead_weight\n",
            "r:\n",
            "  return: '${incoming.body.name}'\n",
            "  next: end\n",
        ),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    let finding = body["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "declaration.body.over_declared")
        .expect("over_declared not found");
    assert_eq!(finding["fields"], serde_json::json!(["dead_weight"]));
    assert_eq!(finding["severity"], "warning");
}

#[tokio::test]
async fn audit_endpoint_internal_missing_is_info_severity() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/pub.yml",
        concat!(
            "declaration:\n",
            "  description: 'public'\n",
            "r:\n",
            "  return: ok\n",
            "  next: end\n",
        ),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    let findings = body["findings"].as_array().unwrap();
    let f = findings
        .iter()
        .find(|f| f["code"] == "declaration.internal_missing")
        .expect("internal_missing not found");
    assert_eq!(f["severity"], "info");
}

#[tokio::test]
async fn audit_endpoint_totals_match_findings_counts() {
    let tmp = TempDir::new().unwrap();
    // Two DSLs: one with declaration.missing (warning), one clean enough
    // to only trigger declaration.internal_missing (info).
    write_dsl(
        tmp.path(),
        "svc/GET/no-decl.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    write_dsl(
        tmp.path(),
        "svc/GET/with-decl.yml",
        concat!(
            "declaration:\n",
            "  description: 'has'\n",
            "  returns: []\n",
            "r:\n",
            "  return: ok\n",
            "  next: end\n",
        ),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    let findings = body["findings"].as_array().unwrap();
    let errors = findings.iter().filter(|f| f["severity"] == "error").count();
    let warnings = findings
        .iter()
        .filter(|f| f["severity"] == "warning")
        .count();
    let info = findings.iter().filter(|f| f["severity"] == "info").count();
    assert_eq!(body["totals"]["errors"].as_u64().unwrap() as usize, errors);
    assert_eq!(
        body["totals"]["warnings"].as_u64().unwrap() as usize,
        warnings
    );
    assert_eq!(body["totals"]["info"].as_u64().unwrap() as usize, info);
}

#[tokio::test]
async fn audit_endpoint_findings_sorted_by_project_dsl_code() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "beta/GET/thing.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    write_dsl(
        tmp.path(),
        "alpha/GET/thing.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let body = audit_body(router).await;
    let findings = body["findings"].as_array().unwrap();
    assert!(findings.len() >= 2);
    // Both "alpha" findings must come before all "beta" findings.
    let first_beta_idx = findings
        .iter()
        .position(|f| f["project"] == "beta")
        .unwrap();
    let last_alpha_idx = findings
        .iter()
        .rposition(|f| f["project"] == "alpha")
        .unwrap();
    assert!(
        last_alpha_idx < first_beta_idx,
        "alpha findings must precede beta findings"
    );
}

#[tokio::test]
async fn audit_endpoint_not_on_public_router() {
    // Mirror shape of security_hardening.rs — public router does NOT
    // mount `/_/audit/dsl`. Only admin_router does.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/ping.yml",
        concat!("r:\n", "  return: ok\n", "  next: end\n"),
    );
    let router = try_build_router(tmp.path()).unwrap();
    let req = Request::builder()
        .method("GET")
        .uri("/_/audit/dsl")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    // Public router routes `/_/*` to the DSL handler which returns 404.
    assert_eq!(
        resp.status(),
        404,
        "public router must NOT expose /_/audit/dsl"
    );
}

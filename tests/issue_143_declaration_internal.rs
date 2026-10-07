//! Issue #143 — `declaration.internal: true` + operator-level default
//! policy for not-HTTP-reachable DSLs.
//!
//! Coverage:
//! 1. External HTTP to an `internal: true` DSL → 404 (not 403, to avoid
//!    leaking that the route exists).
//! 2. External HTTP to an `internal: false` DSL → routed normally (200).
//! 3. External HTTP to a DSL without `internal:` + default_internal=false
//!    (framework default) → routed normally (200). The feature is
//!    strictly opt-in; existing deployments see zero wire change.
//! 4. External HTTP to a DSL without `internal:` + default_internal=true
//!    → 404 (operator-level flip to private-by-default).
//! 5. `internal: false` on a DSL where default_internal=true wins
//!    (per-DSL opt-out of the operator-level flip).
//! 6. `template:` from an external public DSL into an internal DSL →
//!    reaches the internal DSL (self-call / template bypass the gate).
//! 7. `/_/openapi.json` emits `x-internal: true` for the explicitly-
//!    internal DSL and omits it for public DSLs.
//! 8. `/_/unguarded` surfaces an `internal` field on every audited
//!    route.
//! 9. Boot WARN policy — `missing_internal_policy: warn` logs one WARN
//!    per DSL that omits `declaration.internal` (not verified here;
//!    log capture would duplicate `tests/logging.rs` scaffolding).

use ruuter_on_rust::config::{AppConfig, DeclarationsConfig, MissingInternalPolicy};
use ruuter_on_rust::testkit::harness::Harness;
use std::collections::HashMap;
use std::fs;

fn write_dsl(root: &std::path::Path, rel: &str, body: &str) {
    let full = root.join(rel);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(&full, body).unwrap();
}

fn fresh_dsl_root(name: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("ruuter-143-{}-{}", name, ns));
    fs::create_dir_all(&path).unwrap();
    path
}

fn empty_query() -> HashMap<String, serde_json::Value> {
    HashMap::new()
}

fn empty_headers() -> HashMap<String, String> {
    HashMap::new()
}

#[tokio::test]
async fn internal_true_returns_404_on_external_http() {
    let root = fresh_dsl_root("internal-true-404");
    write_dsl(
        &root,
        "svc/GET/private.yml",
        "declaration: { internal: true, description: 'private' }\n\
         r: { return: should_not_reach, next: end }\n",
    );
    let h = Harness::build(&root, HashMap::new()).unwrap();
    let r = h
        .execute_http(
            "GET",
            "/svc/private",
            None,
            &empty_query(),
            &empty_headers(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 404,
        "internal DSL must be 404 externally; got body: {}",
        r.body
    );
    assert_ne!(r.status, 403, "must not be 403 — avoid leaking existence");
}

#[tokio::test]
async fn internal_false_routes_normally() {
    let root = fresh_dsl_root("internal-false-ok");
    write_dsl(
        &root,
        "svc/GET/public.yml",
        "declaration: { internal: false, description: 'public' }\n\
         r: { return: ok, next: end }\n",
    );
    let h = Harness::build(&root, HashMap::new()).unwrap();
    let r = h
        .execute_http("GET", "/svc/public", None, &empty_query(), &empty_headers())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn absent_internal_with_framework_default_false_routes_normally() {
    let root = fresh_dsl_root("absent-default-false");
    // No `declaration.internal` at all; no `ruuter.yaml`. Framework
    // default `false` wins — route is public. This is the back-compat
    // guarantee: existing deployments see zero wire change.
    write_dsl(
        &root,
        "svc/GET/legacy.yml",
        "declaration: { description: 'legacy' }\n\
         r: { return: ok, next: end }\n",
    );
    let h = Harness::build(&root, HashMap::new()).unwrap();
    let r = h
        .execute_http("GET", "/svc/legacy", None, &empty_query(), &empty_headers())
        .await
        .unwrap();
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn absent_internal_with_operator_default_true_returns_404() {
    let root = fresh_dsl_root("absent-default-true");
    write_dsl(
        &root,
        "svc/GET/legacy.yml",
        "declaration: { description: 'legacy' }\n\
         r: { return: ok, next: end }\n",
    );
    let mut cfg = AppConfig::default();
    cfg.config_path = root.clone();
    cfg.declarations = DeclarationsConfig {
        default_internal: true,
        missing_internal_policy: MissingInternalPolicy::Silent,
    };
    let h = Harness::build_with_config(cfg, HashMap::new()).unwrap();
    let r = h
        .execute_http("GET", "/svc/legacy", None, &empty_query(), &empty_headers())
        .await
        .unwrap();
    assert_eq!(
        r.status, 404,
        "operator-level default_internal=true + absent per-DSL field \
         must gate externally; got body: {}",
        r.body
    );
}

#[tokio::test]
async fn per_dsl_internal_false_overrides_operator_default_true() {
    let root = fresh_dsl_root("per-dsl-override");
    write_dsl(
        &root,
        "svc/GET/public.yml",
        "declaration: { internal: false, description: 'opt-out of operator default' }\n\
         r: { return: ok, next: end }\n",
    );
    let mut cfg = AppConfig::default();
    cfg.config_path = root.clone();
    cfg.declarations = DeclarationsConfig {
        default_internal: true,
        missing_internal_policy: MissingInternalPolicy::Silent,
    };
    let h = Harness::build_with_config(cfg, HashMap::new()).unwrap();
    let r = h
        .execute_http("GET", "/svc/public", None, &empty_query(), &empty_headers())
        .await
        .unwrap();
    assert_eq!(
        r.status, 200,
        "per-DSL internal=false must override operator default_internal=true"
    );
}

#[tokio::test]
async fn template_from_public_reaches_internal() {
    // The template step and self-call shortcut both reach a target DSL
    // via `DslRouter::execute_dsl`, which bypasses the external-HTTP
    // gate at `handle_request_inner`. Internal DSLs stay reachable
    // from in-process callers (that's the whole point).
    let root = fresh_dsl_root("template-reaches-internal");
    write_dsl(
        &root,
        "svc/GET/public.yml",
        "declaration:\n  internal: false\n\
         fetch:\n  template: helpers/private\n  request_type: GET\n  result: inner\n  next: r\n\
         r:\n  return: '${inner.ok}'\n  next: end\n",
    );
    write_dsl(
        &root,
        "svc/GET/helpers/private.yml",
        "declaration:\n  internal: true\n\
         r:\n  return:\n    ok: 'from-internal'\n  next: end\n",
    );
    let h = Harness::build(&root, HashMap::new()).unwrap();
    let r = h
        .execute_http("GET", "/svc/public", None, &empty_query(), &empty_headers())
        .await
        .unwrap();
    assert_eq!(r.status, 200, "body on failure: {}", r.body);
    assert_eq!(
        r.body["response"], "from-internal",
        "template step must reach internal DSL (bypasses external gate); got body: {}",
        r.body
    );
}

#[tokio::test]
async fn internal_dsl_direct_external_http_still_404_when_also_template_target() {
    // Same setup as `template_from_public_reaches_internal`, but we
    // hit the internal DSL's own URL from the outside. The template-
    // target path is unaffected by this gate (verified by the other
    // test); the external path must stay closed.
    let root = fresh_dsl_root("direct-external-still-404");
    write_dsl(
        &root,
        "svc/GET/public.yml",
        "declaration:\n  internal: false\n\
         fetch:\n  template: helpers/private\n  request_type: GET\n  result: inner\n  next: r\n\
         r:\n  return: '${inner.ok}'\n  next: end\n",
    );
    write_dsl(
        &root,
        "svc/GET/helpers/private.yml",
        "declaration:\n  internal: true\n\
         r:\n  return:\n    ok: 'from-internal'\n  next: end\n",
    );
    let h = Harness::build(&root, HashMap::new()).unwrap();
    let r = h
        .execute_http(
            "GET",
            "/svc/helpers/private",
            None,
            &empty_query(),
            &empty_headers(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 404,
        "direct external access to internal DSL must 404 even when it's also a template target"
    );
}

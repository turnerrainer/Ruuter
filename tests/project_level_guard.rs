//! Integration tests for the project-level guard (issue #39).
//!
//! A `.guard[.yml|.yaml]` file at the project root applies to every
//! HTTP method in the project. Runs as the outermost guard, before
//! any method-scoped ancestor guards. A method-scoped guard with
//! `declaration.override_ancestors: true` replaces every ancestor,
//! project-level included.

// Test-fixture AppConfig assembly. See tests/trigger_dispatch.rs for
// rationale.
#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-proj-guard-{}", uuid()));
    for (rel_path, body) in files {
        let p = tmp.join(rel_path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws_registry.clone());
    DslRouter::new(
        loaded.http,
        loaded.guards,
        cfg,
        StateStore::new(),
        ws_registry,
        engine,
    )
}

/// Try to build a router. Returns the loader error text if load fails —
/// useful when a test wants to assert that a bad DSL tree is rejected
/// at load time.
fn try_build(files: &[(&str, &str)]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("ruuter-proj-guard-neg-{}", uuid()));
    for (rel_path, body) in files {
        let p = tmp.join(rel_path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    loader
        .load_everything()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

const PROJECT_GUARD_REQUIRES_X_TOKEN: &str = r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: ok
ok:
  return: { passed: project }
  next: end
deny:
  status: 401
  return: { error: "missing x-token" }
  next: end
"#;

/// Baseline — project-level guard fires on every method without needing
/// a per-method guard duplicate. Same DSL body across GET and POST; the
/// project guard rejects both when the header is missing.
#[tokio::test]
async fn project_guard_applies_across_http_methods() {
    let router = build(&[
        ("svc/.guard.yml", PROJECT_GUARD_REQUIRES_X_TOKEN),
        (
            "svc/GET/ping.yml",
            r#"
respond: { return: { via: get }, next: end }
"#,
        ),
        (
            "svc/POST/ping.yml",
            r#"
respond: { return: { via: post }, next: end }
"#,
        ),
    ]);

    // GET without token — project guard denies.
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(r.value.as_ref().unwrap()["error"], "missing x-token");

    // POST without token — same guard fires; not a duplicate .guard.yml.
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 401,
        "project-level guard must fire on POST too — that's the whole point of #39"
    );

    // GET with token — project guard passes; main DSL runs.
    let mut h = HashMap::new();
    h.insert("x-token".into(), "abc".into());
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            h,
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["via"], "get");
}

/// Project-level guard stacks with a method-scoped guard: project runs
/// first (outermost), method guard runs next. Both must pass; project's
/// short-circuit skips both the method guard AND the main DSL.
#[tokio::test]
async fn project_guard_stacks_before_method_scoped_guard() {
    let router = build(&[
        ("svc/.guard.yml", PROJECT_GUARD_REQUIRES_X_TOKEN),
        (
            "svc/POST/admin.guard.yml",
            r#"
check:
  switch:
    - condition: "${incoming.headers['x-role'] !== 'admin'}"
      next: deny
  next: ok
ok:
  return: { passed: admin }
  next: end
deny:
  status: 403
  return: { error: "admin required" }
  next: end
"#,
        ),
        (
            "svc/POST/admin/users.yml",
            r#"
respond: { return: { ok: true }, next: end }
"#,
        ),
    ]);

    // No token → project guard denies with 401; method guard never runs.
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "admin/users",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(r.value.unwrap()["error"], "missing x-token");

    // Token present, wrong role → project passes, method denies with 403.
    let mut h = HashMap::new();
    h.insert("x-token".into(), "abc".into());
    h.insert("x-role".into(), "user".into());
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "admin/users",
            HashMap::new(),
            HashMap::new(),
            h,
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 403,
        "method-scoped guard fires after project passes"
    );

    // Both pass → main DSL runs.
    let mut h = HashMap::new();
    h.insert("x-token".into(), "abc".into());
    h.insert("x-role".into(), "admin".into());
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "admin/users",
            HashMap::new(),
            HashMap::new(),
            h,
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["ok"], true);
}

/// `override_ancestors: true` on a nested guard replaces EVERY outer
/// guard, project-level included. Public-under-otherwise-protected
/// pattern: project requires auth, one endpoint opts out entirely.
#[tokio::test]
async fn method_guard_override_ancestors_bypasses_project_guard() {
    let router = build(&[
        ("svc/.guard.yml", PROJECT_GUARD_REQUIRES_X_TOKEN),
        (
            "svc/GET/public.guard.yml",
            r#"
declaration:
  override_ancestors: true
allow:
  return: { public: true }
  next: end
"#,
        ),
        (
            "svc/GET/public/hello.yml",
            r#"
respond: { return: { hello: world }, next: end }
"#,
        ),
    ]);

    // No project-guard token, but override guard fires and passes.
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "public/hello",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 200,
        "override_ancestors on the inner guard must bypass the project guard"
    );
    assert_eq!(r.value.unwrap()["hello"], "world");
}

/// A project without a `.guard*` file behaves exactly as before (no
/// regression). Sanity check that the extra load-path branch does not
/// leak a phantom guard into projects that don't opt in.
#[tokio::test]
async fn project_without_guard_file_runs_main_dsl_unprotected() {
    let router = build(&[(
        "svc/GET/ping.yml",
        r#"
respond: { return: { pong: true }, next: end }
"#,
    )]);

    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "ping",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["pong"], true);
}

/// Two project-level guard filename variants at once (`.guard.yml`
/// alongside `.guard.yaml`) is ambiguous — the loader must refuse
/// rather than silently picking one based on file-system iteration
/// order. Naming both offending files in the error message lets an
/// operator fix it without diffing.
#[test]
fn multiple_project_level_guard_files_is_a_load_error() {
    let err = try_build(&[
        ("svc/.guard.yml", PROJECT_GUARD_REQUIRES_X_TOKEN),
        ("svc/.guard.yaml", PROJECT_GUARD_REQUIRES_X_TOKEN),
        (
            "svc/GET/ping.yml",
            r#"
respond: { return: pong, next: end }
"#,
        ),
    ])
    .expect_err("loading with two project-level guard files must fail");
    assert!(
        err.contains("project 'svc'") && err.contains(".guard.yml") && err.contains(".guard.yaml"),
        "load error must name the project and both offending files; got: {err}"
    );
}

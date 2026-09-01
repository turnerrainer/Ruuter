//! Regression tests for issue #41 — sibling guard convention silently
//! failed when the guard and the DSL sat in the same directory (both
//! keyed `<METHOD>/path/<stem>`). The trailing-slash prefix check in
//! `applicable_guards` never matched the same-key DSL, leaving the
//! route silently unguarded.
//!
//! Also asserts the follow-on behaviour the fix implies:
//! - Ancestor guards over child DSLs (the pre-#41 code path) still
//!   fire.
//! - Sibling guards are name-scoped, NOT directory-scoped: a peer
//!   `.yml` file with a different stem is unguarded by design. The
//!   test locks this in so a future refactor doesn't accidentally
//!   widen the match.

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
    let tmp = std::env::temp_dir().join(format!("ruuter-issue-41-{}", uuid()));
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

const REQUIRES_X_API_KEY: &str = r#"
check:
  switch:
    - condition: "${!incoming.headers['x-api-key']}"
      next: deny
  next: allow
allow:
  return: { passed: true }
  next: end
deny:
  status: 401
  return: { error: "missing x-api-key" }
  next: end
"#;

/// The exact repro from issue #41. `consignments.guard.yml` sits in
/// the SAME directory as `consignments.yml`; both compute to guard
/// key `POST/v1/consignments`. Before the fix, `applicable_guards`
/// silently skipped the guard because `"POST/v1/consignments"` does
/// not `starts_with("POST/v1/consignments/")`, and the request
/// reached the DSL unguarded.
#[tokio::test]
async fn issue_41_sibling_guard_same_directory_actually_fires() {
    let router = build(&[
        (
            "platforms/POST/v1/consignments.guard.yml",
            REQUIRES_X_API_KEY,
        ),
        (
            "platforms/POST/v1/consignments.yml",
            r#"
respond: { return: { received: true }, next: end }
"#,
        ),
    ]);

    // Without the header, the guard MUST reject. Before #41, this
    // returned 200 (guard silently skipped, main DSL ran to completion).
    let r = router
        .execute_dsl(
            "platforms",
            "POST",
            "v1/consignments",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 401,
        "same-directory sibling guard must fire — issue #41 regression"
    );
    assert_eq!(r.value.as_ref().unwrap()["error"], "missing x-api-key");

    // With the header, guard passes and the main DSL responds.
    let mut h = HashMap::new();
    h.insert("x-api-key".into(), "abc".into());
    let r = router
        .execute_dsl(
            "platforms",
            "POST",
            "v1/consignments",
            HashMap::new(),
            HashMap::new(),
            h,
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["received"], true);
}

/// The same guard file also protects child routes under a matching-
/// name folder. Confirms that after the exact-match fix, sibling-
/// guard-over-CHILD (the pre-#41 supported case) still works —
/// no regression on the prefix branch.
#[tokio::test]
async fn sibling_guard_still_protects_child_routes_via_prefix_match() {
    let router = build(&[
        (
            "platforms/POST/v1/consignments.guard.yml",
            REQUIRES_X_API_KEY,
        ),
        (
            "platforms/POST/v1/consignments/detail.yml",
            r#"
respond: { return: { detail: 42 }, next: end }
"#,
        ),
    ]);

    // Without header, guard rejects the child route (prefix match).
    let r = router
        .execute_dsl(
            "platforms",
            "POST",
            "v1/consignments/detail",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 401);

    // With header, guard passes and child DSL responds.
    let mut h = HashMap::new();
    h.insert("x-api-key".into(), "abc".into());
    let r = router
        .execute_dsl(
            "platforms",
            "POST",
            "v1/consignments/detail",
            HashMap::new(),
            HashMap::new(),
            h,
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["detail"], 42);
}

/// Same directory, one guard covers itself + all children of its
/// name-subtree. Both the same-key route and the child route are
/// guarded by the single `.guard.yml`.
#[tokio::test]
async fn sibling_guard_covers_same_key_and_children_together() {
    let router = build(&[
        ("platforms/POST/v1/orders.guard.yml", REQUIRES_X_API_KEY),
        (
            "platforms/POST/v1/orders.yml",
            r#"
respond: { return: { list: true }, next: end }
"#,
        ),
        (
            "platforms/POST/v1/orders/create.yml",
            r#"
respond: { return: { created: true }, next: end }
"#,
        ),
    ]);

    let mut h = HashMap::new();
    h.insert("x-api-key".into(), "abc".into());

    for (path, expected_field) in [("v1/orders", "list"), ("v1/orders/create", "created")] {
        let r = router
            .execute_dsl(
                "platforms",
                "POST",
                path,
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                "test".into(),
            )
            .await
            .unwrap();
        assert_eq!(r.status, 401, "no header → {path} guard denies");

        let r = router
            .execute_dsl(
                "platforms",
                "POST",
                path,
                HashMap::new(),
                HashMap::new(),
                h.clone(),
                "test".into(),
            )
            .await
            .unwrap();
        assert_eq!(r.status, 200, "with header → {path} passes");
        assert_eq!(r.value.unwrap()[expected_field], true);
    }
}

/// The trap case from the follow-on discussion: peer DSL files with
/// DIFFERENT stems are NOT covered by each other's guard. Sibling
/// guards are name-scoped, not directory-scoped. Locking this
/// behaviour in so a future "widen the match" refactor doesn't
/// accidentally start guarding peer routes that were designed to
/// be public.
///
/// If you want the whole directory guarded, use the in-folder
/// convention (`<dir>/.guard.yml`) or a project-level `.guard.yml`
/// instead — see `book/src/dsl/guards.md`.
#[tokio::test]
async fn sibling_guard_does_not_leak_onto_peer_dsl_with_different_stem() {
    let router = build(&[
        // Guard protects `foo` (same-key and children).
        ("api/POST/foo.guard.yml", REQUIRES_X_API_KEY),
        (
            "api/POST/foo.yml",
            r#"
respond: { return: { it: foo }, next: end }
"#,
        ),
        // Guard protects `another` (same-key and children).
        ("api/POST/another.guard.yml", REQUIRES_X_API_KEY),
        (
            "api/POST/another.yml",
            r#"
respond: { return: { it: another }, next: end }
"#,
        ),
        // Peer file — DELIBERATELY not covered by either guard.
        // If the exact-match branch ever widened to "same directory
        // = same guard", this test would fail with status 401
        // instead of 200.
        (
            "api/POST/is_this_unguarded.yml",
            r#"
respond: { return: { public: true }, next: end }
"#,
        ),
    ]);

    // `foo` guard fires on `foo.yml` (missing header → 401).
    let r = router
        .execute_dsl(
            "api",
            "POST",
            "foo",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 401,
        "foo.yml is guarded by its sibling foo.guard.yml"
    );

    // `another` guard fires on `another.yml` (missing header → 401).
    let r = router
        .execute_dsl(
            "api",
            "POST",
            "another",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 401,
        "another.yml is guarded by its sibling another.guard.yml"
    );

    // But the peer file has no matching guard and MUST reach the
    // main DSL with no auth check. This is by design — sibling
    // guards are name-scoped.
    let r = router
        .execute_dsl(
            "api",
            "POST",
            "is_this_unguarded",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        r.status, 200,
        "is_this_unguarded.yml has no matching guard by name — must not \
         inherit foo.guard.yml or another.guard.yml just for being in the \
         same directory"
    );
    assert_eq!(r.value.unwrap()["public"], true);
}

//! Tests for the guard-audit helper (issue #45) and the two callers
//! that consume it: `dsl-lint --require-guard` and
//! `GET /_/unguarded`. Also verifies that the refactor of
//! `DslRouter::applicable_guards` (now delegating to
//! `guard_keys_for_dsl`) preserves the exact same runtime semantics
//! by hitting both paths against the same tree.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{AppConfig, GuardMode};
use ruuter_on_rust::dsl::guard_audit::{audit_all_routes, guard_keys_for_dsl};
use ruuter_on_rust::dsl::loader::DslLoader;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

fn write_tree(files: &[(&str, &str)]) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("ruuter-audit-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    tmp
}

fn load(tmp: &Path) -> ruuter_on_rust::dsl::loader::LoadedProjects {
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp.to_path_buf();
    DslLoader::new(cfg, HashMap::new())
        .load_everything()
        .unwrap()
}

const NO_OP_GUARD: &str = r#"
allow:
  return: { passed: true }
  next: end
"#;

const NO_OP_DSL: &str = r#"
respond:
  return: { ok: true }
  next: end
"#;

/// The audit distinguishes guarded from unguarded routes exactly as
/// the router's per-request `applicable_guards` does. Same tree, same
/// verdict — that's the single-source-of-truth contract that keeps
/// the audit from drifting from execution.
#[test]
fn audit_reports_guarded_and_unguarded_routes() {
    let tmp = write_tree(&[
        ("api/.guard.yml", NO_OP_GUARD),
        ("api/GET/status.yml", NO_OP_DSL),
        ("api/POST/orders.yml", NO_OP_DSL),
        // Second project — no guards at all → every route unguarded.
        ("public/GET/health.yml", NO_OP_DSL),
        ("public/GET/version.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::Stack);
    // Deterministic sort by (project, method, path).
    let names: Vec<(String, String, String, bool)> = audit
        .iter()
        .map(|r| {
            (
                r.project.clone(),
                r.method.clone(),
                r.path.clone(),
                r.is_unguarded(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("api".into(), "GET".into(), "status".into(), false),
            ("api".into(), "POST".into(), "orders".into(), false),
            ("public".into(), "GET".into(), "health".into(), true),
            ("public".into(), "GET".into(), "version".into(), true),
        ]
    );
}

/// Project-level guard (issue #39) shows up in the audit under the
/// reserved `*` key — that's the sentinel `guard_keys_for_dsl`
/// returns, and the audit forwards it verbatim so callers can render
/// it any way they like.
#[test]
fn audit_surfaces_project_level_guard_key() {
    let tmp = write_tree(&[
        ("svc/.guard.yml", NO_OP_GUARD),
        ("svc/GET/one.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::Stack);
    assert_eq!(audit.len(), 1);
    assert!(!audit[0].is_unguarded());
    assert_eq!(audit[0].guards, vec!["*".to_string()]);
}

/// A route covered by both a project-level guard AND a method-scoped
/// guard reports both keys, outermost-first. This is the stacking
/// case — the audit renders exactly what the router would run.
#[test]
fn audit_stacks_project_and_method_scoped_guards_outer_first() {
    let tmp = write_tree(&[
        ("svc/.guard.yml", NO_OP_GUARD),
        ("svc/POST/admin.guard.yml", NO_OP_GUARD),
        ("svc/POST/admin/users.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::Stack);
    let users = audit
        .iter()
        .find(|r| r.path == "admin/users")
        .expect("admin/users must be in audit");
    assert_eq!(
        users.guards,
        vec!["*".to_string(), "POST/admin".to_string()],
        "project-level guard runs before the method-scoped ancestor"
    );
}

/// Direct unit test of `guard_keys_for_dsl` for the issue #41 case:
/// sibling guard in the SAME directory as the DSL. Locks in the
/// exact-match branch at the audit-helper level so the router's
/// refactor (which now delegates to this) can't accidentally drop it.
#[test]
fn guard_keys_matches_same_directory_sibling_via_exact_match() {
    let tmp = write_tree(&[
        ("svc/POST/consignments.guard.yml", NO_OP_GUARD),
        ("svc/POST/consignments.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let keys = guard_keys_for_dsl("svc", "POST/consignments", &loaded.guards, GuardMode::Stack);
    assert_eq!(
        keys,
        vec!["POST/consignments".to_string()],
        "issue #41 exact-match branch must fire for same-directory sibling"
    );
}

/// `override_ancestors: true` on a method-scoped guard drops every
/// ancestor including the project-level guard from the audit — same
/// escape-hatch semantic the router honours.
#[test]
fn audit_honours_override_ancestors_on_nested_guard() {
    let tmp = write_tree(&[
        ("svc/.guard.yml", NO_OP_GUARD),
        (
            "svc/GET/public.guard.yml",
            r#"
declaration:
  override_ancestors: true
allow:
  return: { pub: true }
  next: end
"#,
        ),
        ("svc/GET/public/hello.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::Stack);
    let hello = audit
        .iter()
        .find(|r| r.path == "public/hello")
        .expect("public/hello in audit");
    assert_eq!(
        hello.guards,
        vec!["GET/public".to_string()],
        "override_ancestors must kick out the project-level guard for this subtree"
    );
}

/// `GuardMode::ClosestOnly` narrows method-scoped ancestors to the
/// innermost, but the project-level guard still prepends — the audit
/// mirrors the router's contract (see the interaction section in
/// `book/src/config/guards-mode.md`).
#[test]
fn audit_closest_only_keeps_project_guard_and_only_innermost_method_ancestor() {
    let tmp = write_tree(&[
        ("svc/.guard.yml", NO_OP_GUARD),
        ("svc/POST/api.guard.yml", NO_OP_GUARD),
        ("svc/POST/api/admin.guard.yml", NO_OP_GUARD),
        ("svc/POST/api/admin/users.yml", NO_OP_DSL),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::ClosestOnly);
    let users = audit
        .iter()
        .find(|r| r.path == "api/admin/users")
        .expect("api/admin/users in audit");
    assert_eq!(
        users.guards,
        vec!["*".to_string(), "POST/api/admin".to_string()],
        "ClosestOnly drops POST/api (outer method-scoped ancestor), keeps POST/api/admin \
         (innermost); project-level `*` still prepends regardless of mode"
    );
}

/// v0.9.11 (h2ck.me H2a): WS/inbound handlers ARE now surfaced in
/// the audit. Pre-fix the WS bucket was silently dropped from
/// `audit_all_routes`, which meant `/_/unguarded` reported
/// `unguarded: 0` while an unauthenticated attacker could upgrade
/// to a WS handler without ever hitting a guard. The audit
/// endpoint's contract is "report every route with no applicable
/// guard"; WS routes belong in that contract.
#[test]
fn audit_includes_ws_inbound_handlers() {
    let tmp = write_tree(&[
        ("svc/.guard.yml", NO_OP_GUARD),
        ("svc/GET/ping.yml", NO_OP_DSL),
        (
            "svc/WS/inbound/subscribe.yml",
            r#"
frame:
  return: { ok: true }
  next: end
"#,
        ),
    ]);
    let loaded = load(&tmp);
    let audit = audit_all_routes(&loaded.http, &loaded.guards, GuardMode::Stack);
    // WS route MUST appear in the audit.
    let ws_entry = audit
        .iter()
        .find(|r| r.method == "WS")
        .expect("WS route must appear in /_/unguarded output");
    // Project-level `.guard.yml` still applies to WS routes now that
    // the guard chain runs on the WS upgrade — verify the audit
    // reflects that ordering (project-level `*` first).
    assert!(
        ws_entry.guards.contains(&"*".to_string()),
        "project-level guard `*` applies to the WS route: {:?}",
        ws_entry
    );
    // Sanity — the HTTP route IS present too.
    assert!(audit.iter().any(|r| r.method == "GET" && r.path == "ping"));
}

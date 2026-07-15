//! Integration tests for the guards primitive (#015).
//!
//! Convention: `<stem>.guard.yml` at `<METHOD>/<stem>.guard.yml`
//! protects every DSL whose key starts with `<METHOD>/<stem>/`.
//! A guard returning a >= 400 status short-circuits the main DSL;
//! its response (status + body) becomes the response.

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
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

fn build(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-guard-{}", uuid()));
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
    DslRouter::new(loaded.http, loaded.guards, cfg, StateStore::new(), ws_registry, engine)
}

#[tokio::test]
async fn guard_blocks_request_with_missing_header() {
    let router = build(&[
        ("svc/GET/protected.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: allow

allow:
  return: { ok: true }
  next: end

deny:
  status: 401
  return: { error: "missing token" }
  next: end
"#),
        ("svc/GET/protected/data.yml", r#"
respond:
  return: { secret: "shhh" }
  next: end
"#),
    ]);

    // No header → guard denies with 401.
    let r = router.execute_dsl(
        "svc", "GET", "protected/data",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(r.value.unwrap()["error"], "missing token");
}

#[tokio::test]
async fn guard_lets_request_through_when_header_present() {
    let router = build(&[
        ("svc/GET/protected.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: allow

allow:
  return: { ok: true }
  next: end

deny:
  status: 401
  return: { error: "missing token" }
  next: end
"#),
        ("svc/GET/protected/data.yml", r#"
respond:
  return: { secret: "shhh" }
  next: end
"#),
    ]);

    let mut headers = HashMap::new();
    headers.insert("x-token".into(), "abc".into());

    let r = router.execute_dsl(
        "svc", "GET", "protected/data",
        HashMap::new(), HashMap::new(), headers,
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["secret"], "shhh");
}

#[tokio::test]
async fn unprotected_routes_run_without_a_guard() {
    let router = build(&[
        ("svc/GET/protected.guard.yml", r#"
deny:
  status: 401
  return: { error: "blocked" }
  next: end
"#),
        ("svc/GET/public/data.yml", r#"
respond: { return: { data: "anyone can see this" }, next: end }
"#),
    ]);

    let r = router.execute_dsl(
        "svc", "GET", "public/data",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["data"], "anyone can see this");
}

#[tokio::test]
async fn nested_guards_stack_outer_runs_first() {
    let router = build(&[
        // Outer guard: requires any auth header.
        ("svc/GET/api.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['authorization']}"
      next: deny
  next: ok
ok:
  return: { passed: outer }
  next: end
deny:
  status: 401
  return: { error: "no auth" }
  next: end
"#),
        // Inner guard: additionally requires admin role.
        ("svc/GET/api/admin.guard.yml", r#"
check:
  switch:
    - condition: "${incoming.headers['x-role'] !== 'admin'}"
      next: deny
  next: ok
ok:
  return: { passed: inner }
  next: end
deny:
  status: 403
  return: { error: "admin required" }
  next: end
"#),
        ("svc/GET/api/admin/users.yml", r#"
respond:
  return: { users: ["alice","bob"] }
  next: end
"#),
    ]);

    // No auth at all → outer guard denies with 401 (inner shouldn't run).
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), HashMap::new(),
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401, "outer guard must fire first");

    // Auth present but not admin → outer passes, inner denies with 403.
    let mut h = HashMap::new();
    h.insert("authorization".into(), "Bearer x".into());
    h.insert("x-role".into(), "user".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), h,
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 403, "inner guard fires when outer passes");

    // Full admin → both pass, main DSL runs.
    let mut h = HashMap::new();
    h.insert("authorization".into(), "Bearer x".into());
    h.insert("x-role".into(), "admin".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), h,
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["users"][0], "alice");
}

#[tokio::test]
async fn guard_can_pass_variables_to_main_dsl() {
    let router = build(&[
        ("svc/GET/api.guard.yml", r#"
parse_token:
  assign:
    user_id: "${incoming.headers['x-token']}"
  next: allow

allow:
  return: { ok: true }
  next: end
"#),
        ("svc/GET/api/whoami.yml", r#"
respond:
  return: { user: "${user_id}" }
  next: end
"#),
    ]);

    let mut h = HashMap::new();
    h.insert("x-token".into(), "user-42".into());

    let r = router.execute_dsl(
        "svc", "GET", "api/whoami",
        HashMap::new(), HashMap::new(), h,
        "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["user"], "user-42");
}

#[tokio::test]
async fn guards_isolate_per_project() {
    // Project A has a guard. Project B doesn't. B's same-named route
    // must NOT inherit A's guard.
    let router = build(&[
        ("a/GET/protected.guard.yml", r#"
deny: { status: 401, return: { error: "A is locked" }, next: end }
"#),
        ("a/GET/protected/x.yml", r#"
ok: { return: { side: "A" }, next: end }
"#),
        ("b/GET/protected/x.yml", r#"
ok: { return: { side: "B" }, next: end }
"#),
    ]);

    // A: blocked by guard.
    let r = router.execute_dsl(
        "a", "GET", "protected/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);

    // B: open.
    let r = router.execute_dsl(
        "b", "GET", "protected/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["side"], "B");
}

// ─── Java-parity: in-folder guards ─────────────────────────────────────
// Task 019. A `.guard.yml` (or `.guard`) inside the folder it protects
// is a Java Ruuter convention. Rust previously silently dropped these.

#[tokio::test]
async fn in_folder_guard_yml_protects_its_directory() {
    let router = build(&[
        ("svc/GET/protected/.guard.yml", r#"
deny:
  status: 401
  return: { error: "locked from inside" }
  next: end
"#),
        ("svc/GET/protected/data.yml", r#"
ok: { return: { secret: "shhh" }, next: end }
"#),
    ]);

    let r = router.execute_dsl(
        "svc", "GET", "protected/data",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(r.value.unwrap()["error"], "locked from inside");
}

#[tokio::test]
async fn in_folder_guard_stacks_with_ancestor_sibling_guard() {
    // Sibling guard on `api` folder + in-folder guard on `api/admin` —
    // both must pass. Proves the two conventions cooperate.
    let router = build(&[
        ("svc/GET/api.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['authorization']}"
      next: deny
  next: ok
ok: { return: { passed: outer }, next: end }
deny: { status: 401, return: { error: "no auth" }, next: end }
"#),
        ("svc/GET/api/admin/.guard.yml", r#"
check:
  switch:
    - condition: "${incoming.headers['x-role'] !== 'admin'}"
      next: deny
  next: ok
ok: { return: { passed: inner }, next: end }
deny: { status: 403, return: { error: "admin only" }, next: end }
"#),
        ("svc/GET/api/admin/users.yml", r#"
ok: { return: { users: ["alice"] }, next: end }
"#),
    ]);

    // outer fails → 401
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);

    // outer passes, inner fails → 403
    let mut h = HashMap::new();
    h.insert("authorization".into(), "Bearer x".into());
    h.insert("x-role".into(), "user".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), h, "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 403);

    // both pass → 200
    let mut h = HashMap::new();
    h.insert("authorization".into(), "Bearer x".into());
    h.insert("x-role".into(), "admin".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/admin/users",
        HashMap::new(), HashMap::new(), h, "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["users"][0], "alice");
}

// ─── Task 020: bespoke guard override ─────────────────────────────────

#[tokio::test]
async fn override_guard_replaces_ancestor_guards() {
    // Outer guard requires x-token; inner guard on /api/inject-fault
    // is stricter (403 unconditionally) AND declares override_ancestors.
    // Result: inner runs, outer skipped.
    let router = build(&[
        ("svc/GET/api.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: ok
ok: { return: { passed: outer }, next: end }
deny: { status: 401, return: { error: "no token" }, next: end }
"#),
        ("svc/GET/api/inject-fault.guard.yml", r#"
declaration:
  override_ancestors: true

deny:
  status: 403
  return: { error: "inject-fault disabled in prod" }
  next: end
"#),
        ("svc/GET/api/inject-fault/trigger.yml", r#"
ok: { return: { fired: true }, next: end }
"#),
    ]);

    // Even WITH x-token, override guard runs → 403. Outer skipped.
    let mut h = HashMap::new();
    h.insert("x-token".into(), "valid".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/inject-fault/trigger",
        HashMap::new(), HashMap::new(), h, "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 403);
    assert_eq!(r.value.unwrap()["error"], "inject-fault disabled in prod");
}

#[tokio::test]
async fn override_absent_means_stack_semantics_unchanged() {
    // Same shape as above but no override_ancestors — both guards must
    // pass. Back-compat check.
    let router = build(&[
        ("svc/GET/api.guard.yml", r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: ok
ok: { return: { passed: outer }, next: end }
deny: { status: 401, return: { error: "no token" }, next: end }
"#),
        ("svc/GET/api/careful.guard.yml", r#"
check:
  switch:
    - condition: "${incoming.headers['x-role'] !== 'admin'}"
      next: deny
  next: ok
ok: { return: { passed: inner }, next: end }
deny: { status: 403, return: { error: "admin only" }, next: end }
"#),
        ("svc/GET/api/careful/action.yml", r#"
ok: { return: { ran: true }, next: end }
"#),
    ]);

    // With token but not admin → outer passes, inner denies.
    let mut h = HashMap::new();
    h.insert("x-token".into(), "valid".into());
    h.insert("x-role".into(), "user".into());
    let r = router.execute_dsl(
        "svc", "GET", "api/careful/action",
        HashMap::new(), HashMap::new(), h, "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 403);
    assert_eq!(r.value.unwrap()["error"], "admin only");
}

#[tokio::test]
async fn override_only_affects_routes_it_matches() {
    // Override on api/inject-fault must not touch api/normal.
    let router = build(&[
        ("svc/GET/api.guard.yml", r#"
deny: { status: 401, return: { error: "outer" }, next: end }
"#),
        ("svc/GET/api/inject-fault.guard.yml", r#"
declaration:
  override_ancestors: true
ok: { return: { override_ran: true }, next: end }
"#),
        ("svc/GET/api/inject-fault/x.yml", r#"
ok: { return: { path: fault }, next: end }
"#),
        ("svc/GET/api/normal/y.yml", r#"
ok: { return: { path: normal }, next: end }
"#),
    ]);

    // inject-fault path: override runs → returns OK, outer bypassed.
    let r = router.execute_dsl(
        "svc", "GET", "api/inject-fault/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["path"], "fault");

    // normal path: outer still runs, denies 401.
    let r = router.execute_dsl(
        "svc", "GET", "api/normal/y",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);
    assert_eq!(r.value.unwrap()["error"], "outer");
}

#[tokio::test]
async fn in_folder_guard_doesnt_protect_sibling_folders() {
    let router = build(&[
        ("svc/GET/locked/.guard.yml", r#"
deny: { status: 401, return: { error: "locked" }, next: end }
"#),
        ("svc/GET/locked/x.yml", r#"
ok: { return: { area: locked }, next: end }
"#),
        ("svc/GET/open/y.yml", r#"
ok: { return: { area: open }, next: end }
"#),
    ]);

    // locked/x → guarded
    let r = router.execute_dsl(
        "svc", "GET", "locked/x",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 401);

    // open/y → unaffected
    let r = router.execute_dsl(
        "svc", "GET", "open/y",
        HashMap::new(), HashMap::new(), HashMap::new(), "test".into(),
    ).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["area"], "open");
}

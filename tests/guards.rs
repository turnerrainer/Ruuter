//! Integration tests for the guards primitive (#015).
//!
//! Convention: `<stem>.guard.yml` at `<METHOD>/<stem>.guard.yml`
//! protects every DSL whose key starts with `<METHOD>/<stem>/`.
//! A guard returning a >= 400 status short-circuits the main DSL;
//! its response (status + body) becomes the response.

use ruuter_rs::config::AppConfig;
use ruuter_rs::dsl::loader::DslLoader;
use ruuter_rs::router::DslRouter;
use ruuter_rs::state::StateStore;
use ruuter_rs::ws::WsRegistry;
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
    DslRouter::new(loaded.http, loaded.guards, cfg, StateStore::new(), WsRegistry::new())
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

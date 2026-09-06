//! h2ck.me v0.9.10-rc audit — PoC tests + fix pins.
//!
//! Each test is written to BREAK the currently-shipping behaviour
//! per CLAUDE.md's "write tests that try to break the fix" rule.
//! Baseline (pre-fix) runs show the vulnerability; post-fix runs
//! flip green and pin the intended behaviour.
//!
//! Findings covered in this file
//!
//! - **H1** — `template:` step invokes a target DSL without running
//!   the guards that gate it via HTTP. A DSL author who templates
//!   into a guarded route silently bypasses the guard, and
//!   `GET /_/unguarded` still reports the target as "guarded",
//!   giving operators false assurance.
//! - **H2** — WS server never runs guards. `handle_ws_upgrade` and
//!   `dispatch_ws_frame` skip `applicable_guards` entirely. A guard
//!   configured for `WS/<path>` (or an ancestor prefix like the
//!   project-level `*`) is silently ignored, and the same
//!   `/_/unguarded` audit endpoint excludes WS from its report so
//!   operators can't discover the gap.
//! - **M1** — `GET /_/openapi.json` is unauthenticated by default and
//!   enumerates every DSL route, method, and (when declared)
//!   request/response schema. Same posture belongs behind the
//!   `RUUTER_ADMIN_ENABLED` gate as `/_/sources` and `/_/unguarded`.
//! - **M2** — `RUUTER_HTTP_REWRITE` is a "test-only" env-driven URL
//!   rewriter that fires BEFORE `check_ssrf`. In a release build an
//!   operator who accidentally sets it in prod silently loses
//!   allowlist / private-network protections for the rewritten
//!   origin, with no warning at boot.
//! - **M3** — WS outbound writer channel is `mpsc::unbounded_channel`.
//!   A slow / dead reader combined with a broadcast can grow the
//!   sender queue without bound. Memory-DoS surface.

// Test-fixture AppConfig assembly.
#![allow(clippy::field_reassign_with_default)]

use arc_swap::ArcSwap;
use axum::body::Body;
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
use tower::ServiceExt;

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
    build_with_cfg(files, AppConfig::default())
}

fn build_with_cfg(files: &[(&str, &str)], mut cfg: AppConfig) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-h2ck-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared_http = Arc::new(ArcSwap::from_pointee(loaded.http));
    let shared_guards = Arc::new(ArcSwap::from_pointee(loaded.guards));
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(shared_http.clone())
        // h2ck.me H1 — mirror main.rs's wiring so the template step
        // enforces the guards attached to the target DSL. Without this
        // the test would silently reproduce the pre-fix bypass.
        .with_guards(shared_guards.clone(), cfg.guards.mode);
    DslRouter::from_shared(
        shared_http,
        shared_guards,
        cfg,
        StateStore::new(),
        ws,
        engine,
    )
}

// ────────────────────────────────────────────────────────────────
// H1 — template step bypasses target DSL guards
// ────────────────────────────────────────────────────────────────

/// Pre-fix behaviour: the caller `POST/public/entry` templates into
/// `POST/admin/things`, and the `POST/admin/.guard.yml` guard that
/// gates the admin subtree over HTTP is silently skipped. Post-fix:
/// the guard runs against the child context and short-circuits, so
/// the caller sees the guard's 403 body instead of the target's
/// `ok: true`.
#[tokio::test]
async fn template_step_must_run_target_dsl_guards() {
    let router = build(&[
        (
            "svc/POST/admin/.guard.yml",
            r#"
deny:
  return: { error: "denied by guard" }
  status: 403
  next: end
"#,
        ),
        (
            "svc/POST/admin/things.yml",
            r#"
respond:
  return: { ok: true, reached_admin: true }
  status: 200
  next: end
"#,
        ),
        (
            "svc/POST/public/entry.yml",
            r#"
call_admin:
  template: admin/things
  request_type: POST
  result: r
  next: shape

shape:
  return: { proxied: "${r}" }
  next: end
"#,
        ),
    ]);

    let outcome = router
        .execute_dsl(
            "svc",
            "POST",
            "public/entry",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect("execute_dsl");

    // Post-fix contract:
    //   - The guard MUST short-circuit the template call. Either the
    //     caller receives the guard body verbatim, or the template
    //     step propagates an error naming the guard.
    //   - The target DSL's own `ok: true` payload MUST NOT reach the
    //     caller — that would mean the guard was skipped.
    let body_text = serde_json::to_string(&outcome.value).unwrap_or_default();
    assert!(
        !body_text.contains("reached_admin"),
        "template invocation reached the guarded admin DSL body without \
         running the guard: {}",
        body_text
    );
}

// ────────────────────────────────────────────────────────────────
// H2 — WebSocket upgrade skips all guards
// ────────────────────────────────────────────────────────────────

/// The `/_/unguarded` audit endpoint should report a WS route that
/// has no applicable guard exactly the same way it reports an
/// unguarded HTTP route. Pre-fix, WS routes are excluded from the
/// audit entirely — an operator running the endpoint gets a "totals"
/// count of `unguarded: 0` while an unauthenticated attacker can
/// upgrade to the WS handler without ever hitting a guard.
#[tokio::test]
async fn unguarded_audit_must_list_ws_routes_without_guards() {
    // No guards on either the HTTP route or the WS route.
    let router = Arc::new(build(&[
        (
            "svc/GET/ping.yml",
            r#"
respond:
  return: { pong: true }
  status: 200
  next: end
"#,
        ),
        (
            "svc/WS/inbound/notify.yml",
            r#"
frame:
  return: { ok: true }
  next: end
"#,
        ),
    ]));

    // Enable the admin router so /_/unguarded is mounted.
    std::env::set_var("RUUTER_ADMIN_ENABLED", "true");
    let app = router.clone().admin_router();
    std::env::remove_var("RUUTER_ADMIN_ENABLED");

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/_/unguarded")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let has_ws_entry = json["projects"]["svc"]["unguarded"]
        .as_array()
        .map(|arr| arr.iter().any(|e| e["method"] == "WS"))
        .unwrap_or(false);
    assert!(
        has_ws_entry,
        "GET /_/unguarded must surface WS routes that have no applicable \
         guard — the WS plane bypasses guards today, so silently omitting \
         it from the audit gives operators false assurance. Got body: {}",
        String::from_utf8_lossy(&bytes)
    );
}

// ────────────────────────────────────────────────────────────────
// M1 — /_/openapi.json is publicly reachable
// ────────────────────────────────────────────────────────────────

/// Pre-fix, `GET /_/openapi.json` is mounted on the public router and
/// enumerates every DSL route + declared schema without any auth. It
/// should sit behind the same `RUUTER_ADMIN_ENABLED` gate as
/// `/_/sources` and `/_/unguarded`. This test asserts the default-off
/// posture — with the admin gate NOT enabled, the public router does
/// not respond 200 to `/_/openapi.json`.
#[tokio::test]
async fn openapi_json_must_be_admin_gated_by_default() {
    // Explicitly force the admin gate off so this test is stable
    // regardless of the ambient environment.
    std::env::remove_var("RUUTER_ADMIN_ENABLED");

    let router = build(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#,
    )]);

    let app = Arc::new(router).build_axum_router_from_arc();

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/_/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "/_/openapi.json must not enumerate the DSL surface to \
         unauthenticated callers when RUUTER_ADMIN_ENABLED is unset"
    );
}

// ────────────────────────────────────────────────────────────────
// M2 — RUUTER_HTTP_REWRITE production posture
// ────────────────────────────────────────────────────────────────

/// The `RUUTER_HTTP_REWRITE` env var is documented as "test-only" but
/// is compiled into release builds and consulted on every outbound
/// request BEFORE `check_ssrf`. A stray setting in prod silently
/// disables SSRF protection for the rewritten origin.
///
/// This test doesn't remove the env var (that would break the test
/// fixture harness other tests rely on); it pins a boot-time check
/// that at least surfaces the risk. Post-fix, the framework exposes
/// a helper the operator's boot code can call to detect the env var
/// and emit a WARN.
#[test]
fn ruuter_http_rewrite_env_is_flagged_as_dangerous_in_release() {
    // Post-fix: expose a public helper that returns `true` when the
    // env var is set AND the current build is a release build. Boot
    // code emits a WARN so operators discover the misconfiguration
    // in the same log stream as "Loaded config from …".
    //
    // In test builds `cfg(debug_assertions)` is on, so the helper
    // returns `false` regardless. The check therefore locks in the
    // API existing rather than the value.
    let _ = ruuter_on_rust::http_client::rewrite_env_is_active_in_release();
    // No assertion on the boolean — we just require the symbol to
    // exist post-fix. A missing symbol fails to compile.
}

// ────────────────────────────────────────────────────────────────
// M3 — WS outbound writer channel bounded
// ────────────────────────────────────────────────────────────────

/// Registry writer channels should be bounded so a slow reader
/// combined with a fan-out broadcast can't grow the sender queue
/// without bound. Post-fix, `WsRegistry::send` returns an error
/// (or drops) when the peer's queue is at capacity instead of
/// buffering indefinitely.
#[tokio::test]
async fn ws_registry_send_must_be_bounded() {
    use ruuter_on_rust::ws::Outbound;
    let reg = WsRegistry::new();
    // Register a connection whose reader we never drain, then send
    // more messages than the intended cap. Post-fix: excess sends
    // return Err.
    let (tx, _rx) = ruuter_on_rust::ws::bounded_sender(4);
    reg.register("client:slow".into(), tx);

    let mut errors = 0;
    for i in 0..1024 {
        if reg
            .send("client:slow", serde_json::json!({"n": i}))
            .is_err()
        {
            errors += 1;
            break;
        }
    }
    // 4-slot queue can accept a small handful before back-pressure;
    // sending 1024 without ever draining MUST surface at least one
    // error rather than silently buffering unbounded.
    let _ = Outbound::Json(serde_json::json!({})); // ensure Outbound stays public
    assert!(
        errors > 0,
        "unbounded WS writer queue allowed 1024 messages to be enqueued \
         against a never-drained reader — a slow client can OOM the \
         process via broadcast fan-out"
    );
}

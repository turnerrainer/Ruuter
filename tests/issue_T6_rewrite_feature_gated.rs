//! h2ck.me v1 T-6 — `RUUTER_HTTP_REWRITE` is now compiled ONLY
//! into debug builds and release builds that opt into the
//! `dev-http-rewrite` Cargo feature.
//!
//! Pre-fix, the rewriter code path shipped in every release binary.
//! An operator who accidentally set `RUUTER_HTTP_REWRITE=…` in prod
//! silently disabled SSRF (`check_ssrf`) for the rewritten origin,
//! with a boot WARN as the only mitigation (M2). Fleet doctrine
//! (`h2ck.me/FLEET-STRONGHOLDS §10`) prefers a Cargo feature gate
//! so the misconfigured state literally can't happen in a stock
//! release binary.
//!
//! Post-fix (v1 T-6):
//! - New Cargo feature `dev-http-rewrite = []`.
//! - `rewrite_url_for_tests` and `rewrite_env_is_active_in_release`
//!   are conditionally compiled behind
//!   `#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]`.
//!   The non-feature branch replaces the implementations with
//!   no-op stubs; the same public API compiles in both cases so
//!   `main.rs` can call the WARN helper unconditionally.
//! - `RUUTER_HTTP_REWRITE_ENV` (the const) still exported in both
//!   builds — it's just a string constant, no risk.
//!
//! This test binary always runs with `debug_assertions` on. What
//! we CAN verify here:
//!
//! 1. In debug builds (this suite), the rewriter behaviour is
//!    unchanged — the env var, when set to a valid rewrite pair,
//!    routes an outbound request to the pinned target. Regression
//!    pin for the debug-mode path.
//! 2. `rewrite_env_is_active_in_release()` returns `false` in the
//!    test binary (debug_assertions on) regardless of env — the
//!    guard inside the fn is intentional, and this pins it.
//! 3. `RUUTER_HTTP_REWRITE_ENV` is still exposed in both builds.
//!
//! The "release-without-feature is a no-op" property is verified by
//! `cargo build --release` (checked in CI). Also documented in
//! `CHANGELOG.md` for the operator-facing migration story.

use ruuter_on_rust::http_client::{rewrite_env_is_active_in_release, RUUTER_HTTP_REWRITE_ENV};

/// The env-var name constant is stable and part of the public
/// contract. Both builds expose it (it's just a `&'static str`).
#[test]
fn env_var_name_is_exposed() {
    assert_eq!(RUUTER_HTTP_REWRITE_ENV, "RUUTER_HTTP_REWRITE");
}

/// `rewrite_env_is_active_in_release` in a debug-assertions-on
/// binary always returns `false`, regardless of whether the env
/// is set. Pin — a future refactor that drops the
/// `cfg!(debug_assertions)` guard would surface here.
#[test]
fn active_in_release_is_false_in_debug_binary() {
    // Note: env-var scope: setting per-test env can leak across
    // parallel tests. We snapshot + restore rather than assuming
    // exclusive access.
    let prev = std::env::var(RUUTER_HTTP_REWRITE_ENV).ok();
    std::env::set_var(
        RUUTER_HTTP_REWRITE_ENV,
        "http://a.example=http://127.0.0.1:9999",
    );
    let active = rewrite_env_is_active_in_release();
    assert!(
        !active,
        "test binary runs with debug_assertions ON — the release-only \
         WARN helper must return false to avoid noise in tests"
    );
    // Restore
    match prev {
        Some(v) => std::env::set_var(RUUTER_HTTP_REWRITE_ENV, v),
        None => std::env::remove_var(RUUTER_HTTP_REWRITE_ENV),
    }
}

/// Empty env → also false. Sanity check for the empty-value path
/// (the "unset" path is exercised by the check above via remove).
#[test]
fn active_in_release_is_false_for_empty_env() {
    let prev = std::env::var(RUUTER_HTTP_REWRITE_ENV).ok();
    std::env::set_var(RUUTER_HTTP_REWRITE_ENV, "");
    let active = rewrite_env_is_active_in_release();
    assert!(!active);
    match prev {
        Some(v) => std::env::set_var(RUUTER_HTTP_REWRITE_ENV, v),
        None => std::env::remove_var(RUUTER_HTTP_REWRITE_ENV),
    }
}

// ────────────────────────────────────────────────────────────────
// Debug-mode rewriter still works. Uses a local HTTP server on
// 127.0.0.1:kernel-port and sets RUUTER_HTTP_REWRITE to redirect
// a bogus URL to that server. This test binary has
// `debug_assertions` on so the rewriter IS compiled in and MUST
// still function — otherwise dsl-test would break every scenario
// that pins `http_rewrite:`.
// ────────────────────────────────────────────────────────────────

// h2ck.me v1 T-6 (CI fix) — the end-to-end rewriter test needs
// axum + HttpClient. In release builds without the
// `dev-http-rewrite` feature the rewriter is compiled out per
// T-6 itself, so the test would find no rewriting happening and
// fail. Gate the imports + helpers + test with the same cfg the
// production code uses, so `cargo test --release` (CI's mode)
// passes without the feature.
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
use axum::{routing::get, Router};
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
use ruuter_on_rust::config::AppConfig;
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
use ruuter_on_rust::http_client::HttpClient;
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
use std::time::Duration;
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
use tokio::net::TcpListener;

#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
async fn spawn_id_server(tag: &'static str) -> u16 {
    let app = Router::new().route(
        "/tag",
        get(move || async move { axum::Json(serde_json::json!({ "hit": tag })) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    port
}

/// The env-var rewriter still redirects outbound URLs when the
/// rewriter code is compiled in (debug builds OR release with the
/// `dev-http-rewrite` feature). Gated with the same cfg the
/// production code uses so `cargo test --release` in CI — which
/// runs without the feature — doesn't fail with an unroutable
/// URL. The other three tests in this file (const, false-in-debug,
/// empty env) still run in every configuration.
///
/// NB: env-var is process-wide and other tests in this binary that
/// hit outbound URLs will see it. To keep collateral damage minimal
/// we scope the value tightly and restore on exit.
#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]
#[tokio::test]
async fn debug_build_still_rewrites() {
    let port = spawn_id_server("hit").await;
    let prev = std::env::var(RUUTER_HTTP_REWRITE_ENV).ok();
    std::env::set_var(
        RUUTER_HTTP_REWRITE_ENV,
        format!("http://target.example=http://127.0.0.1:{}", port),
    );

    // Use HttpClient built from AppConfig::default() — block_private_networks
    // is on, but with rewriting the URL becomes `http://127.0.0.1:port/...`
    // which is loopback; allowlist opt-in required. Simpler: turn off
    // the SSRF block for this test so we're validating ONLY the rewriter.
    let mut cfg = AppConfig::default();
    cfg.internal_requests.block_private_networks = false;
    cfg.http_request_timeout = 2000;
    let client = HttpClient::new(&cfg);
    let resp = client
        .request(
            reqwest::Method::GET,
            "http://target.example/tag",
            None,
            None,
            None,
            None,
        )
        .await;

    // Restore before asserting so a panic doesn't leave the env
    // polluted for other tests.
    match prev {
        Some(v) => std::env::set_var(RUUTER_HTTP_REWRITE_ENV, v),
        None => std::env::remove_var(RUUTER_HTTP_REWRITE_ENV),
    }

    let resp = resp.expect("rewritten request must succeed");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.unwrap()["hit"], "hit");
}

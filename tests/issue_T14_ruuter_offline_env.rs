//! h2ck.me v1 T-14 — `RUUTER_OFFLINE=true` hard-stubs every outbound.
//!
//! `dsl-test` already has a MockServer + per-test `http_rewrite:`
//! for hermetic integration testing; production and staging boot
//! had no analogue. When set, every outbound `http.*` step short-
//! circuits BEFORE `check_ssrf` / connect / UDS with the same stub
//! shape issue #89 introduced for transport errors:
//!
//! ```json
//! { "status": 0, "error": "offline",
//!   "body": { "error": "offline", "message": "..." },
//!   "headers": {} }
//! ```
//!
//! DSL `check_*` switches that key on `${result.response.status == 0}`
//! (issue #89's escape hatch) fire in offline mode too — no DSL
//! changes needed to make a production DSL testable offline.
//!
//! `main.rs` emits a boot WARN whenever the env is set so ops
//! teams don't confuse offline-mode zero-status responses for a
//! real upstream outage.
//!
//! Tests written to try to BREAK the fix:
//! - Env unset → normal outbound path (dispatch attempted).
//! - Env=true → stub returned, status 0, error "offline".
//! - Env=1 → stub returned (truthy variant).
//! - Env=yes → stub returned.
//! - Env=false → normal outbound path (not truthy).
//! - Env=empty string → normal outbound path.
//! - Stub body shape matches contract.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::http_client::{
    offline_stub_response, ruuter_offline_env_active, HttpClient, RUUTER_OFFLINE_ENV,
};
use std::time::Duration;

fn client() -> HttpClient {
    let mut cfg = AppConfig::default();
    cfg.http_request_timeout = 500;
    HttpClient::new(&cfg)
}

/// Process-wide mutex serialising every env-mutating test in this
/// binary. tokio's default is multithreaded runtime + cargo runs
/// tests in parallel, so plain snapshot/restore isn't enough — a
/// parallel thread's env read races with our write. Every test
/// that touches RUUTER_OFFLINE acquires this lock.
use std::sync::{Mutex, MutexGuard, OnceLock};
fn env_mutex() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

/// Snapshot + restore around env-var mutation to avoid leaking state
/// into other parallel tests in the same binary. Holds a mutex
/// while the guard is alive.
struct EnvGuard {
    prev: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(v: &str) -> Self {
        let lock = env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(RUUTER_OFFLINE_ENV).ok();
        std::env::set_var(RUUTER_OFFLINE_ENV, v);
        Self { prev, _lock: lock }
    }
    fn unset() -> Self {
        let lock = env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(RUUTER_OFFLINE_ENV).ok();
        std::env::remove_var(RUUTER_OFFLINE_ENV);
        Self { prev, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(RUUTER_OFFLINE_ENV, v),
            None => std::env::remove_var(RUUTER_OFFLINE_ENV),
        }
    }
}

// ────────────────────────────────────────────────────────────────
// ruuter_offline_env_active — the truthy-value classifier
// ────────────────────────────────────────────────────────────────

#[test]
fn env_unset_is_not_active() {
    let _g = EnvGuard::unset();
    assert!(!ruuter_offline_env_active());
}

#[test]
fn env_empty_is_not_active() {
    let _g = EnvGuard::set("");
    assert!(!ruuter_offline_env_active());
}

#[test]
fn env_false_is_not_active() {
    let _g = EnvGuard::set("false");
    assert!(!ruuter_offline_env_active());
}

#[test]
fn env_zero_is_not_active() {
    let _g = EnvGuard::set("0");
    assert!(!ruuter_offline_env_active());
}

#[test]
fn env_true_is_active() {
    let _g = EnvGuard::set("true");
    assert!(ruuter_offline_env_active());
}

#[test]
fn env_true_case_insensitive() {
    let _g = EnvGuard::set("TRUE");
    assert!(ruuter_offline_env_active());
}

#[test]
fn env_one_is_active() {
    let _g = EnvGuard::set("1");
    assert!(ruuter_offline_env_active());
}

#[test]
fn env_yes_is_active() {
    let _g = EnvGuard::set("yes");
    assert!(ruuter_offline_env_active());
}

#[test]
fn env_on_is_active() {
    let _g = EnvGuard::set("on");
    assert!(ruuter_offline_env_active());
}

#[test]
fn env_junk_is_not_active() {
    // Random string that isn't a truthy variant — treat as unset.
    // Prevents "RUUTER_OFFLINE=please" accidentally enabling.
    let _g = EnvGuard::set("please");
    assert!(!ruuter_offline_env_active());
}

// ────────────────────────────────────────────────────────────────
// offline_stub_response shape
// ────────────────────────────────────────────────────────────────

#[test]
fn stub_status_is_zero() {
    let r = offline_stub_response();
    assert_eq!(r.status, 0);
}

#[test]
fn stub_error_field_is_offline() {
    let r = offline_stub_response();
    assert_eq!(r.error.as_deref(), Some("offline"));
}

#[test]
fn stub_body_contains_error_marker() {
    let r = offline_stub_response();
    let body = r.body.expect("body");
    assert_eq!(body["error"], "offline");
    assert!(body["message"].as_str().unwrap().contains("RUUTER_OFFLINE"));
}

#[test]
fn stub_headers_are_empty() {
    let r = offline_stub_response();
    assert!(r.headers.is_empty());
}

// ────────────────────────────────────────────────────────────────
// End-to-end: with the env set, `HttpClient::request` returns
// the stub without ever dialling.
//
// NB: tests in the same binary share process env. These tests use
// EnvGuard to restore state. Running with `--test-threads=1` is
// not required; the guard snapshots on entry and restores on drop.
// But test parallelism means we shouldn't assume other tests won't
// see our env mutations mid-test — so we scope tightly and use
// clearly-nonresolvable target URLs so a leaked "on" state in a
// parallel test would still fail-fast rather than hit the network.
// ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn client_returns_stub_when_offline() {
    let _g = EnvGuard::set("true");
    let c = client();
    let resp = c
        .request(
            reqwest::Method::GET,
            // Deliberately garbage — should never be dialled.
            "http://does-not-exist.invalid.example:1/x",
            None,
            None,
            None,
            Some(Duration::from_millis(200)),
        )
        .await
        .expect("offline stub is Ok");
    assert_eq!(resp.status, 0);
    assert_eq!(resp.error.as_deref(), Some("offline"));
    assert_eq!(resp.body.as_ref().unwrap()["error"], "offline");
}

#[tokio::test]
async fn client_makes_real_call_when_env_unset() {
    let _g = EnvGuard::unset();
    let c = client();
    // Unresolvable name — the client attempts to dial. With
    // `block_private_networks: true` (default) the check_ssrf
    // DNS-lookup path runs first and rejects with `Err(HttpRequest)`
    // when DNS fails. Either way, the result is NOT the offline
    // stub. That's the invariant we care about here.
    let result = c
        .request(
            reqwest::Method::GET,
            "http://does-not-exist.invalid.example:1/x",
            None,
            None,
            None,
            Some(Duration::from_millis(500)),
        )
        .await;
    match result {
        Ok(resp) => {
            // In-band #89 transport-error stub — must NOT be "offline".
            assert_ne!(
                resp.error.as_deref(),
                Some("offline"),
                "with env unset the error must classify the transport failure, \
                 not the offline stub; got: {:?}",
                resp.error
            );
        }
        Err(e) => {
            // check_ssrf-path rejection is also acceptable. Pin
            // that the message doesn't come from the offline path
            // (which lives on the request_with_ct entry, above
            // check_ssrf).
            let msg = format!("{e}");
            assert!(
                !msg.contains("offline"),
                "err message must not reference offline mode when env is unset; \
                 got: {msg}"
            );
        }
    }
}

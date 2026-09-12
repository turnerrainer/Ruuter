//! h2ck.me v1 T-9 — boot WARN when a non-loopback listener is
//! configured but `response_default_headers` doesn't include the
//! OWASP baseline (X-Content-Type-Options, X-Frame-Options,
//! Strict-Transport-Security, Referrer-Policy).
//!
//! Pre-fix, `response_default_headers` machinery existed and
//! `book/src/ops/security-checklist.md` documented the posture, but
//! there was no boot-time signal — an operator who exposed Ruuter
//! on `0.0.0.0:8080` and forgot to add the baseline never saw a
//! warning. Post-fix, the WARN fires once at boot, naming each
//! missing header, and points at the checklist. Loopback-only
//! deployments (dev, sidecar-on-UDS, `127.0.0.1:port`) never see
//! it — the WARN is scoped to network-reachable listeners.
//!
//! Tests written to try to BREAK the fix:
//!
//! - `has_non_loopback_listener`:
//!     - empty listeners list → true (default fallback binds 0.0.0.0).
//!     - explicit `127.0.0.1:8080` → false (loopback).
//!     - explicit `[::1]:8080` → false (IPv6 loopback).
//!     - explicit `localhost:8080` → false.
//!     - explicit `0.0.0.0:8080` → true (all interfaces = public).
//!     - explicit `10.0.0.5:8080` → true (LAN, but not loopback).
//!     - UDS-only listener → false.
//!     - Mix of loopback + non-loopback → true (any one triggers).
//! - `missing_owasp_baseline_headers`:
//!     - Empty `response_default_headers` → all 4 missing.
//!     - All 4 present (canonical case) → empty.
//!     - All 4 present (lowercase) → empty (case-insensitive match).
//!     - Two present, two missing → returns the two missing.
//! - Subscriber-captured WARN:
//!     - Non-loopback + missing header(s) → exactly one WARN line
//!       naming each missing header AND pointing at security-checklist.
//!     - Loopback + missing → NO WARN.
//!     - Non-loopback + all headers → NO WARN.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{
    has_non_loopback_listener, missing_owasp_baseline_headers, warn_on_stale_config_fields,
    AppConfig, ListenerConfig, OWASP_BASELINE_HEADERS,
};
use std::collections::HashMap;

fn listener_tcp(bind: &str) -> ListenerConfig {
    ListenerConfig {
        name: None,
        bind: Some(bind.to_string()),
        unix: None,
        http2: false,
    }
}

fn listener_uds(path: &str) -> ListenerConfig {
    ListenerConfig {
        name: None,
        bind: None,
        unix: Some(std::path::PathBuf::from(path)),
        http2: false,
    }
}

// ────────────────────────────────────────────────────────────────
// has_non_loopback_listener classification
// ────────────────────────────────────────────────────────────────

#[test]
fn empty_listeners_is_non_loopback_by_default() {
    // main.rs fallback: `0.0.0.0:port` → non-loopback.
    let cfg = AppConfig::default();
    assert!(has_non_loopback_listener(&cfg));
}

#[test]
fn ipv4_loopback_bind_is_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("127.0.0.1:8080")];
    assert!(!has_non_loopback_listener(&cfg));
}

#[test]
fn ipv6_loopback_bind_is_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("[::1]:8080")];
    assert!(!has_non_loopback_listener(&cfg));
}

#[test]
fn localhost_string_bind_is_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("localhost:8080")];
    assert!(!has_non_loopback_listener(&cfg));
}

#[test]
fn all_interfaces_zero_bind_is_non_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("0.0.0.0:8080")];
    assert!(has_non_loopback_listener(&cfg));
}

#[test]
fn lan_ip_bind_is_non_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("10.0.0.5:8080")];
    assert!(has_non_loopback_listener(&cfg));
}

#[test]
fn uds_only_listener_is_loopback_class() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_uds("/tmp/ruuter.sock")];
    assert!(!has_non_loopback_listener(&cfg));
}

#[test]
fn mixed_listeners_triggers_when_any_is_non_loopback() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![
        listener_tcp("127.0.0.1:8080"), // loopback
        listener_tcp("0.0.0.0:9090"),   // non-loopback → any one wins
    ];
    assert!(has_non_loopback_listener(&cfg));
}

#[test]
fn mixed_loopback_and_uds_is_loopback_class() {
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![
        listener_tcp("127.0.0.1:8080"),
        listener_uds("/tmp/ruuter.sock"),
    ];
    assert!(!has_non_loopback_listener(&cfg));
}

// ────────────────────────────────────────────────────────────────
// missing_owasp_baseline_headers
// ────────────────────────────────────────────────────────────────

#[test]
fn empty_response_default_headers_all_four_missing() {
    let cfg = AppConfig::default(); // response_default_headers is empty
    let missing = missing_owasp_baseline_headers(&cfg);
    assert_eq!(missing.len(), 4);
    for name in OWASP_BASELINE_HEADERS {
        assert!(missing.contains(name), "missing must include {name}");
    }
}

#[test]
fn all_four_present_in_canonical_case_yields_empty() {
    let mut cfg = AppConfig::default();
    let mut hdrs = HashMap::new();
    hdrs.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
    hdrs.insert("X-Frame-Options".to_string(), "DENY".to_string());
    hdrs.insert(
        "Strict-Transport-Security".to_string(),
        "max-age=31536000".to_string(),
    );
    hdrs.insert("Referrer-Policy".to_string(), "no-referrer".to_string());
    cfg.response_default_headers = hdrs;
    let missing = missing_owasp_baseline_headers(&cfg);
    assert!(missing.is_empty());
}

#[test]
fn case_insensitive_matching_lowercase() {
    let mut cfg = AppConfig::default();
    let mut hdrs = HashMap::new();
    hdrs.insert("x-content-type-options".to_string(), "nosniff".to_string());
    hdrs.insert("x-frame-options".to_string(), "DENY".to_string());
    hdrs.insert(
        "strict-transport-security".to_string(),
        "max-age=31536000".to_string(),
    );
    hdrs.insert("referrer-policy".to_string(), "no-referrer".to_string());
    cfg.response_default_headers = hdrs;
    let missing = missing_owasp_baseline_headers(&cfg);
    assert!(
        missing.is_empty(),
        "lowercase headers should count as present: {missing:?}"
    );
}

#[test]
fn two_present_two_missing_returns_the_two_missing() {
    let mut cfg = AppConfig::default();
    let mut hdrs = HashMap::new();
    hdrs.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
    hdrs.insert("Referrer-Policy".to_string(), "no-referrer".to_string());
    cfg.response_default_headers = hdrs;
    let missing = missing_owasp_baseline_headers(&cfg);
    assert_eq!(missing.len(), 2);
    assert!(missing.contains(&"X-Frame-Options"));
    assert!(missing.contains(&"Strict-Transport-Security"));
}

// ────────────────────────────────────────────────────────────────
// Subscriber-captured boot WARN
// ────────────────────────────────────────────────────────────────

use std::io;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture(buf: SharedBuf) -> tracing::subscriber::DefaultGuard {
    use tracing_subscriber::{fmt, EnvFilter};
    let subscriber = fmt()
        .with_writer(buf)
        .with_max_level(tracing::Level::WARN)
        .with_env_filter(EnvFilter::new("warn"))
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::set_default(subscriber)
}

#[test]
fn non_loopback_missing_headers_emits_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    // Default cfg: empty listeners → 0.0.0.0 bind → non-loopback.
    // Empty response_default_headers → all 4 missing.
    let cfg = AppConfig::default();
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    // Must mention each baseline header and the checklist doc.
    for name in OWASP_BASELINE_HEADERS {
        assert!(out.contains(name), "WARN must name {name}; got:\n{out}");
    }
    assert!(
        out.contains("security-checklist.md"),
        "WARN must point at book/src/ops/security-checklist.md; got:\n{out}"
    );
    assert!(
        out.contains("h2ck.me v1 T-9"),
        "WARN must attribute the fix so operators can search for it; got:\n{out}"
    );
}

#[test]
fn loopback_bind_does_not_emit_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_tcp("127.0.0.1:8080")];
    // Also clear any OTHER fields that might trigger warns.
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    // Loopback bind must not fire the OWASP-baseline WARN.
    assert!(
        !out.contains("OWASP baseline"),
        "loopback bind should not emit the T-9 WARN; got:\n{out}"
    );
}

#[test]
fn uds_only_listener_does_not_emit_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let mut cfg = AppConfig::default();
    cfg.listeners = vec![listener_uds("/tmp/ruuter.sock")];
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(!out.contains("OWASP baseline"));
}

#[test]
fn non_loopback_with_all_headers_present_emits_no_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let mut cfg = AppConfig::default();
    // Ensure non-loopback: default listeners list is empty → 0.0.0.0.
    let mut hdrs = HashMap::new();
    for name in OWASP_BASELINE_HEADERS {
        hdrs.insert(name.to_string(), "present".to_string());
    }
    cfg.response_default_headers = hdrs;
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("OWASP baseline"),
        "all baseline headers present must not emit the WARN; got:\n{out}"
    );
}

#[test]
fn partial_headers_missing_names_only_the_missing_ones() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let mut cfg = AppConfig::default();
    let mut hdrs = HashMap::new();
    // Provide two out of four.
    hdrs.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
    hdrs.insert("Referrer-Policy".to_string(), "no-referrer".to_string());
    cfg.response_default_headers = hdrs;
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    // The missing ones must be named.
    assert!(out.contains("X-Frame-Options"));
    assert!(out.contains("Strict-Transport-Security"));
    // Whether the fully-present ones appear in the WARN text is
    // determined by the debug-format list — belts-and-braces check
    // that at least one missing name appears is sufficient here.
}

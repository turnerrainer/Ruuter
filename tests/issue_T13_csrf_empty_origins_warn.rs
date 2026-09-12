//! h2ck.me v1 T-13 — boot WARN when `csrf.allowed_origins` is
//! empty.
//!
//! Pre-fix, `csrf.allowed_origins == []` silently disabled the
//! CSRF Origin/Referer check for state-changing methods. The
//! mechanism was documented at `book/src/framework/csrf.md:15` but
//! there was no boot-time signal — an operator who skipped the
//! setup step never saw a warning.
//!
//! Post-fix, `warn_on_stale_config_fields` fires a WARN naming the
//! field and pointing at the doc. Matches the fleet's default-off-
//! warn pattern (stop_in_case_of_exception, RUUTER_HTTP_REWRITE).
//!
//! Tests written to try to BREAK the fix:
//! - Empty `allowed_origins` (default) → WARN emitted.
//! - Populated `allowed_origins` → NO CSRF WARN.
//! - Absent field in YAML → parses as empty → WARN.
//! - Explicit empty list in YAML → parses as empty → WARN.
//! - WARN names `csrf.allowed_origins` and the doc file.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{warn_on_stale_config_fields, AppConfig, CsrfConfig};

// Subscriber pattern reused from issue_92 / T-1 / T-5.
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
fn default_config_emits_csrf_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    // Default cfg: csrf.allowed_origins is empty.
    let cfg = AppConfig::default();
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        out.contains("csrf.allowed_origins"),
        "WARN must name the field; got:\n{out}"
    );
    assert!(
        out.contains("book/src/framework/csrf.md"),
        "WARN must point at the doc; got:\n{out}"
    );
}

#[test]
fn populated_allowed_origins_emits_no_csrf_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let mut cfg = AppConfig::default();
    cfg.csrf = CsrfConfig {
        allowed_origins: vec!["https://app.example.com".to_string()],
        enforce_on_methods: vec!["POST".to_string()],
    };
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("csrf.allowed_origins"),
        "populated origins must not fire the CSRF WARN; got:\n{out}"
    );
}

#[test]
fn absent_csrf_yaml_field_still_warns() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let yaml = "port: 8080\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(out.contains("csrf.allowed_origins"));
}

#[test]
fn explicit_empty_list_in_yaml_still_warns() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let yaml = "csrf:\n  allowed_origins: []\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(out.contains("csrf.allowed_origins"));
}

#[test]
fn warn_fires_exactly_once() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let cfg = AppConfig::default();
    warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    let hits = out.matches("csrf.allowed_origins").count();
    // The whole point of a boot WARN is one line, not spam.
    assert_eq!(
        hits, 1,
        "expected exactly one CSRF WARN, got {hits}:\n{out}"
    );
}

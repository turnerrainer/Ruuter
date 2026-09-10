//! Issue #92 — `stop_in_case_of_exception` should default to `true`
//! when absent from `ruuter.yaml`, and only WARN when explicitly
//! set to `false`.
//!
//! Pre-fix, `#[serde(default)]` on `pub stop_in_case_of_exception:
//! bool` fell back to `Default::default()` for `bool` → `false`,
//! which tripped `warn_on_stale_config_fields` on every boot for
//! operators who never touched the field. The engine's actual
//! behaviour is "always halt on step error" (matches `true`), so
//! the field's absent value was a lie AND a noise generator.
//!
//! Post-fix (option 3 from the ticket): absent → `true` (matches
//! engine behaviour), no WARN. Explicit `false` still WARNs
//! (option 3's intended surface: honest signal that "you set a
//! value we can't honour").

use ruuter_on_rust::config::AppConfig;

/// Absent field → deserialises as `true`. Regression pin: the pre-
/// fix `#[serde(default)]` fell back to `false`.
#[test]
fn absent_field_defaults_to_true() {
    let yaml = r#"
port: 8080
"#;
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert!(
        cfg.stop_in_case_of_exception,
        "absent stop_in_case_of_exception must default to true \
         (matches the engine's actual halt-on-step-error behaviour), \
         got false"
    );
}

/// Empty YAML document — should still deserialise cleanly with all
/// defaults, including `stop_in_case_of_exception: true`.
#[test]
fn empty_document_uses_defaults() {
    let yaml = "{}";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert!(cfg.stop_in_case_of_exception);
}

/// Explicit `true` → stays `true`, no boot WARN.
#[test]
fn explicit_true_stays_true() {
    let yaml = r#"
stop_in_case_of_exception: true
"#;
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert!(cfg.stop_in_case_of_exception);
}

/// Explicit `false` → stays `false`. `warn_on_stale_config_fields`
/// will WARN on this on real boot (see below), but the parsing
/// respects the operator's choice.
#[test]
fn explicit_false_stays_false() {
    let yaml = r#"
stop_in_case_of_exception: false
"#;
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert!(!cfg.stop_in_case_of_exception);
}

/// The Rust-side `AppConfig::default()` also uses `true`, matching
/// the YAML deserialisation default. Kept for code paths that build
/// a config without going through YAML (tests, dsl-lint fixtures).
#[test]
fn appconfig_default_is_true() {
    let cfg = AppConfig::default();
    assert!(cfg.stop_in_case_of_exception);
}

/// `warn_on_stale_config_fields` fires ONLY when the field is
/// explicitly `false`. Verifies by inspecting the config state each
/// call operates against — the fn is a `tracing::warn!` sink and
/// doesn't return a signal, so we test its guard predicate
/// (`!config.stop_in_case_of_exception`) instead.
#[test]
fn warn_guard_only_fires_on_explicit_false() {
    // Absent → true → guard `!true` = false → no WARN.
    let absent: AppConfig = serde_yaml_ng::from_str("{}").unwrap();
    assert!(
        absent.stop_in_case_of_exception,
        "guard should not fire on absent field"
    );

    // Explicit true → true → no WARN.
    let explicit_true: AppConfig =
        serde_yaml_ng::from_str("stop_in_case_of_exception: true").unwrap();
    assert!(explicit_true.stop_in_case_of_exception);

    // Explicit false → false → WARN fires.
    let explicit_false: AppConfig =
        serde_yaml_ng::from_str("stop_in_case_of_exception: false").unwrap();
    assert!(!explicit_false.stop_in_case_of_exception);
}

// ────────────────────────────────────────────────────────────────
// Subscriber-driven tests: assert the actual `tracing::warn!` line
// (or its absence), not just the guard predicate. Uses a thread-
// scoped tracing subscriber that captures every write into a
// shared buffer, then invokes `warn_on_stale_config_fields` and
// inspects what came out.
// ────────────────────────────────────────────────────────────────

use std::io;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// `MakeWriter` that appends every write into a shared `Vec<u8>` so
/// the test can inspect the captured tracing output. Each test
/// builds its own instance; the scoped subscriber (via
/// `tracing::subscriber::set_default`) means parallel test threads
/// don't inherit ours.
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

/// Install `buf` as the tracing sink for the current thread and
/// return a scoped guard. Callers keep the guard alive for the
/// duration of the test body — dropping it restores the previous
/// subscriber so parallel test threads don't inherit ours.
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

/// Absent field → no `stop_in_case_of_exception` line in captured
/// warnings. Pre-#92 this test would have failed: absent field
/// deserialised as `false` and tripped the WARN.
#[test]
fn absent_field_emits_no_warn_line() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let cfg: AppConfig = serde_yaml_ng::from_str("{}").unwrap();
    ruuter_on_rust::config::warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("stop_in_case_of_exception"),
        "absent field must not emit any WARN naming the field; got:\n{out}"
    );
}

/// Explicit `true` → no WARN.
#[test]
fn explicit_true_emits_no_warn_line() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let cfg: AppConfig = serde_yaml_ng::from_str("stop_in_case_of_exception: true").unwrap();
    ruuter_on_rust::config::warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("stop_in_case_of_exception"),
        "explicit true must not emit any WARN naming the field; got:\n{out}"
    );
}

/// Explicit `false` → WARN line that names the field AND explains
/// the honest behaviour. Pin the shape so future wording drift is
/// visible.
#[test]
fn explicit_false_emits_warn_line_naming_the_field() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let cfg: AppConfig = serde_yaml_ng::from_str("stop_in_case_of_exception: false").unwrap();
    ruuter_on_rust::config::warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    assert!(
        out.contains("stop_in_case_of_exception"),
        "explicit false must emit a WARN naming the field; got:\n{out}"
    );
    assert!(
        out.contains("not honoured") || out.contains("not implemented") || out.contains("halts"),
        "WARN must explain the field is inert; got:\n{out}"
    );
    assert!(
        out.contains("WARN"),
        "captured line must be at WARN level; got:\n{out}"
    );
}

/// Exactly one WARN per invocation on explicit-false — not zero,
/// not duplicated. Pins the fire-once contract so a future refactor
/// that moves the check into a per-request path surfaces here.
#[test]
fn explicit_false_emits_exactly_one_warn_line() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let cfg: AppConfig = serde_yaml_ng::from_str("stop_in_case_of_exception: false").unwrap();
    ruuter_on_rust::config::warn_on_stale_config_fields(&cfg);
    drop(_g);
    let out = buf.contents();
    let hits = out.matches("stop_in_case_of_exception").count();
    assert_eq!(
        hits, 1,
        "expected exactly one WARN line naming the field, got {hits}:\n{out}"
    );
}

//! T-1 — `http_response_size_limit` must default to
//! `Some(16 MiB)` when the operator's ruuter.yaml omits the field,
//! and emit a boot WARN when the operator explicitly sets it to
//! `null` (opt-in to uncapped outbound response bodies).
//!
//! Pre-fix, `#[serde(default)]` on `Option<usize>` fell back to
//! `Default::default()` → `None`, silently disabling the outbound
//! response-body cap for every operator whose ruuter.yaml did not
//! set the field. A misbehaving upstream could then OOM the process
//! by returning an oversized body — `HttpClient::request` reads via
//! `response.bytes().await`, which allocates the whole payload.
//!
//! Post-fix (h2ck.me v1 T-1): absent → `Some(16 * 1024 * 1024)`
//! (matches `AppConfig::default()` so operator- and no-config paths
//! converge). Explicit `null` in YAML stays `None` so the opt-in for
//! uncapped is preserved; `warn_on_raw_config_notes` fires a WARN
//! in that case so a mistake (typo, copy-paste of a Java template
//! using null as a sentinel) doesn't slip past review.

use ruuter_on_rust::config::{
    raw_config_notes, warn_on_raw_config_notes, AppConfig, RawConfigNotes,
};

const SIXTEEN_MIB: usize = 16 * 1024 * 1024;

// ────────────────────────────────────────────────────────────────
// Serde-level tests: the wire behaviour of the field.
// ────────────────────────────────────────────────────────────────

/// Absent field → deserialises as `Some(16 MiB)`. This is the
/// regression pin: the pre-fix `#[serde(default)]` on
/// `Option<usize>` fell back to `None`, which was the root of the
/// silently-uncapped-reads bug.
#[test]
fn absent_field_defaults_to_sixteen_mib() {
    let yaml = "port: 8080\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(
        cfg.http_response_size_limit,
        Some(SIXTEEN_MIB),
        "absent http_response_size_limit must default to Some(16 MiB), \
         not None (pre-fix behaviour disabled the cap)"
    );
}

/// Empty YAML document — all defaults, including cap = 16 MiB.
#[test]
fn empty_document_uses_defaults() {
    let cfg: AppConfig = serde_yaml_ng::from_str("{}").expect("parse");
    assert_eq!(cfg.http_response_size_limit, Some(SIXTEEN_MIB));
}

/// Explicit numeric value → stays as set. Operator override wins
/// over the default.
#[test]
fn explicit_numeric_value_stays() {
    let yaml = "http_response_size_limit: 4096\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.http_response_size_limit, Some(4096));
}

/// Large numeric value (bigger than the default) → stays as set.
#[test]
fn explicit_large_numeric_value_stays() {
    let yaml = "http_response_size_limit: 268435456\n"; // 256 MiB
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.http_response_size_limit, Some(256 * 1024 * 1024));
}

/// Zero → stays `Some(0)`. The engine's cap-enforcement path treats
/// zero as "reject any body" (documented at h2ck.me v1 T-8 — same
/// axum `Limited` shape). Not a common operator choice, but the
/// serde surface must round-trip it faithfully.
#[test]
fn explicit_zero_stays_some_zero() {
    let yaml = "http_response_size_limit: 0\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(cfg.http_response_size_limit, Some(0));
}

/// Explicit `null` → deserialises as `None`. This is the escape
/// hatch for operators who need uncapped reads (internal-only
/// deployments). The WARN in `warn_on_raw_config_notes` surfaces
/// the choice at boot; the serde surface just accepts it.
#[test]
fn explicit_null_stays_none() {
    let yaml = "http_response_size_limit: null\n";
    let cfg: AppConfig = serde_yaml_ng::from_str(yaml).expect("parse");
    assert_eq!(
        cfg.http_response_size_limit, None,
        "explicit null must still deserialise as None so the \
         uncapped opt-in works"
    );
}

/// Rust-side `AppConfig::default()` also carries `Some(16 MiB)`.
/// The regression pinned here is: this MUST match
/// `default_http_response_size_limit()`. Prior to T-1, they were
/// out of sync — Self::default() was `Some(16 MiB)` but the serde
/// path skipped it.
#[test]
fn appconfig_default_matches_serde_default() {
    let cfg = AppConfig::default();
    let empty_yaml: AppConfig = serde_yaml_ng::from_str("{}").unwrap();
    assert_eq!(
        cfg.http_response_size_limit,
        empty_yaml.http_response_size_limit
    );
    assert_eq!(cfg.http_response_size_limit, Some(SIXTEEN_MIB));
}

// ────────────────────────────────────────────────────────────────
// RawConfigNotes tests — the null-vs-absent observation that lets
// us fire the WARN only on explicit opt-in, not on omitted field.
// ────────────────────────────────────────────────────────────────

#[test]
fn raw_notes_absent_field_is_not_explicit_null() {
    let yaml = "port: 8080\n";
    let notes = raw_config_notes(yaml);
    assert!(!notes.http_response_size_limit_explicit_null);
}

#[test]
fn raw_notes_empty_document_is_not_explicit_null() {
    let notes = raw_config_notes("{}");
    assert!(!notes.http_response_size_limit_explicit_null);
}

#[test]
fn raw_notes_numeric_value_is_not_explicit_null() {
    let notes = raw_config_notes("http_response_size_limit: 4096\n");
    assert!(!notes.http_response_size_limit_explicit_null);
}

#[test]
fn raw_notes_explicit_null_is_detected() {
    let notes = raw_config_notes("http_response_size_limit: null\n");
    assert!(
        notes.http_response_size_limit_explicit_null,
        "raw scan must detect explicit null so the WARN can fire"
    );
}

#[test]
fn raw_notes_explicit_tilde_shorthand_is_detected() {
    // YAML's `~` is a shorthand for null. Operators sometimes copy-
    // paste this from Java `application.yml` where `~` is common.
    let notes = raw_config_notes("http_response_size_limit: ~\n");
    assert!(
        notes.http_response_size_limit_explicit_null,
        "YAML `~` (null shorthand) must also be detected as \
         explicit null"
    );
}

#[test]
fn raw_notes_field_missing_from_non_map_yaml() {
    // Malformed YAML — top-level is a sequence, not a mapping.
    // Detection must not panic; must return the default (all
    // false). Errors surface elsewhere.
    let notes = raw_config_notes("- a\n- b\n");
    assert!(!notes.http_response_size_limit_explicit_null);
}

#[test]
fn raw_notes_malformed_yaml_returns_default() {
    // Straight-up invalid YAML — must not panic.
    let notes = raw_config_notes(":\n  broken\n :\n");
    assert!(!notes.http_response_size_limit_explicit_null);
}

// ────────────────────────────────────────────────────────────────
// Subscriber-driven tests: assert the actual `tracing::warn!` line
// (or its absence). Pattern reused from issue_92.
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

/// Absent field → no WARN. The whole point of T-1: the pre-fix bug
/// was that operators who never wrote the field got the uncapped
/// behaviour silently. Post-fix: they get the 16 MiB cap AND
/// there's no WARN telling them anything is wrong (because nothing
/// is).
#[test]
fn absent_field_emits_no_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let notes = raw_config_notes("port: 8080\n");
    warn_on_raw_config_notes(&notes);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("http_response_size_limit"),
        "absent field must not emit any WARN naming the field; got:\n{out}"
    );
}

#[test]
fn numeric_field_emits_no_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let notes = raw_config_notes("http_response_size_limit: 8192\n");
    warn_on_raw_config_notes(&notes);
    drop(_g);
    let out = buf.contents();
    assert!(
        !out.contains("http_response_size_limit"),
        "explicit numeric must not emit any WARN; got:\n{out}"
    );
}

/// Explicit null → WARN line naming the field and explaining the
/// OOM risk. Pin the wording gist so drift is visible.
#[test]
fn explicit_null_emits_warn_naming_field() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let notes = raw_config_notes("http_response_size_limit: null\n");
    warn_on_raw_config_notes(&notes);
    drop(_g);
    let out = buf.contents();
    assert!(
        out.contains("http_response_size_limit"),
        "explicit null must emit a WARN naming the field; got:\n{out}"
    );
    assert!(
        out.contains("uncapped") || out.contains("OOM") || out.contains("cap"),
        "WARN must explain the risk; got:\n{out}"
    );
    assert!(
        out.contains("WARN"),
        "captured line must be at WARN level; got:\n{out}"
    );
}

/// Exactly one WARN per invocation on explicit null.
#[test]
fn explicit_null_emits_exactly_one_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let notes = raw_config_notes("http_response_size_limit: null\n");
    warn_on_raw_config_notes(&notes);
    drop(_g);
    let out = buf.contents();
    let hits = out.matches("http_response_size_limit").count();
    assert_eq!(
        hits, 1,
        "expected exactly one WARN naming the field, got {hits}:\n{out}"
    );
}

/// Default-constructed notes → no WARN. Belts-and-braces for the
/// no-config-file boot path (where `load_or_default_with_notes`
/// returns `RawConfigNotes::default()`).
#[test]
fn default_notes_emit_no_warn() {
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    warn_on_raw_config_notes(&RawConfigNotes::default());
    drop(_g);
    let out = buf.contents();
    assert!(
        out.is_empty() || !out.contains("http_response_size_limit"),
        "default notes must not emit any WARN; got:\n{out}"
    );
}

// ────────────────────────────────────────────────────────────────
// End-to-end: the field, once populated by serde with the default,
// reaches the HTTP client cap path. Not a full network test —
// pinning the plumbing here would be an integration-suite scope
// creep. The typed-config surface is what T-1 fixes; the enforcement
// path is separately pinned by the existing http_client tests and
// h2ck.me T-2 (UDS post-hoc cap → Limited).
// ────────────────────────────────────────────────────────────────

/// Serde-round-trip: serialise a default config, deserialise it,
/// verify the cap survives. Not strictly necessary for T-1, but
/// pins the invariant "our default matches our serde default" in
/// the format that actually ships to disk.
#[test]
fn round_trip_default_config_preserves_cap() {
    let cfg = AppConfig::default();
    let yaml = serde_yaml_ng::to_string(&cfg).expect("serialise");
    let back: AppConfig = serde_yaml_ng::from_str(&yaml).expect("deserialise");
    assert_eq!(
        back.http_response_size_limit,
        Some(SIXTEEN_MIB),
        "default → yaml → default must round-trip the cap"
    );
}

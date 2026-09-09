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

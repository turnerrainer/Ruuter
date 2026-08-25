//! Structured-logging behavior tests. Covers what the framework
//! documents in `book/src/logging/` (the operator-facing chapter):
//!
//! - trace_id extraction / generation
//! - redaction of secret-bearing headers and JSON body fields
//! - CRLF stripping (log-injection defence)
//! - body cap
//! - error-chain rendering bounded to 5 hops
//!
//! These are pure-unit-ish tests against the `logging` module so
//! the redaction / cap / trace-id contract regressions surface in
//! CI without booting a server.

use ruuter_on_rust::config::LoggingConfig;
use ruuter_on_rust::logging::{
    cap_and_sanitize, error_chain, redact, render_body_for_log, sanitize_log_value,
    trace_id_from_traceparent,
};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn trace_id_extracted_from_valid_traceparent() {
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    assert_eq!(
        trace_id_from_traceparent(tp),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
}

#[test]
fn trace_id_rejects_malformed_traceparent() {
    assert_eq!(trace_id_from_traceparent(""), None);
    assert_eq!(trace_id_from_traceparent("garbage"), None);
    // 31 hex — too short
    assert_eq!(
        trace_id_from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01"),
        None
    );
    // 33 hex — too long
    assert_eq!(
        trace_id_from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e47360-00f067aa0ba902b7-01"),
        None
    );
}

#[test]
fn sanitize_strips_crlf_from_log_value() {
    assert_eq!(sanitize_log_value("a\nb"), "a b");
    assert_eq!(sanitize_log_value("a\rb"), "a b");
    assert_eq!(sanitize_log_value("a\r\nb"), "a  b");
    assert_eq!(sanitize_log_value("plain"), "plain");
    // Attacker-controlled newline-splicing attempt.
    assert!(!sanitize_log_value("legit\nINFO fake-line").contains('\n'));
}

#[test]
fn body_redaction_top_and_nested() {
    let v = json!({
        "user": "alice",
        "password": "hunter2",
        "profile": { "token": "s3cret", "name": "Alice" }
    });
    let out = redact::redact_json(&v, &["password".into(), "token".into()]);
    assert_eq!(
        out,
        json!({
            "user": "alice",
            "password": "[REDACTED]",
            "profile": { "token": "[REDACTED]", "name": "Alice" }
        })
    );
}

#[test]
fn body_redaction_case_insensitive() {
    let v = json!({ "Authorization": "Bearer x" });
    let out = redact::redact_json(&v, &["authorization".into()]);
    assert_eq!(out.get("Authorization").unwrap(), "[REDACTED]");
}

#[test]
fn body_redaction_walks_arrays() {
    let v = json!({
        "items": [{"secret": "s1"}, {"secret": "s2"}]
    });
    let out = redact::redact_json(&v, &["secret".into()]);
    let items = out.get("items").unwrap().as_array().unwrap();
    assert_eq!(items[0].get("secret").unwrap(), "[REDACTED]");
    assert_eq!(items[1].get("secret").unwrap(), "[REDACTED]");
}

#[test]
fn header_redaction_case_insensitive() {
    let mut h = HashMap::new();
    h.insert("Authorization".to_string(), "Bearer xyz".to_string());
    h.insert("Cookie".to_string(), "session=abc".to_string());
    h.insert("X-Custom".to_string(), "keep".to_string());
    let out = redact::redact_headers(
        &h,
        &["authorization".into(), "cookie".into()],
    );
    assert_eq!(out.get("Authorization").unwrap(), "[REDACTED]");
    assert_eq!(out.get("Cookie").unwrap(), "[REDACTED]");
    assert_eq!(out.get("X-Custom").unwrap(), "keep");
}

#[test]
fn body_cap_truncates_and_marks() {
    let long = "x".repeat(5000);
    let out = cap_and_sanitize(&long, 100);
    assert!(out.len() <= 100 + '…'.len_utf8());
    assert!(out.ends_with('…'));
}

#[test]
fn body_cap_leaves_short_values_alone() {
    assert_eq!(cap_and_sanitize("hi", 100), "hi");
}

#[test]
fn error_chain_bounded_to_five_hops() {
    use std::error::Error;
    use std::fmt;
    #[derive(Debug)]
    struct E {
        msg: &'static str,
        src: Option<Box<E>>,
    }
    impl fmt::Display for E {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{}", self.msg)
        }
    }
    impl Error for E {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.src.as_deref().map(|e| e as &(dyn Error + 'static))
        }
    }
    // 20 nested errors — chain rendering must bound and mark truncation.
    let deep = (0..20).fold(
        E { msg: "leaf", src: None },
        |acc, _| E { msg: "wrap", src: Some(Box::new(acc)) },
    );
    let rendered = error_chain(&deep);
    // Bounded rendering: 5 "caused by" hops + a truncation marker.
    let caused_by_count = rendered.matches("caused by:").count();
    assert_eq!(
        caused_by_count, 6,
        "expected 5 hops + 1 truncation marker, got {}",
        caused_by_count
    );
    assert!(rendered.ends_with("caused by: ..."));
}

#[test]
fn render_body_for_log_honours_redaction_and_cap() {
    let mut cfg = LoggingConfig::default();
    cfg.max_body_bytes = 40;
    cfg.redact_body_fields = vec!["password".into()];
    let v = json!({
        "user": "alice",
        "password": "hunter2"
    });
    let out = render_body_for_log(Some(&v), &cfg);
    assert!(out.contains("[REDACTED]"), "expected redaction: {}", out);
    // Truncation is byte-based; the sentinel is short enough not to
    // hit the cap here, but we still assert the result is bounded.
    assert!(out.len() <= 40 + '…'.len_utf8());
}

#[test]
fn render_body_for_log_none_yields_placeholder() {
    let cfg = LoggingConfig::default();
    assert_eq!(render_body_for_log(None, &cfg), "-");
}

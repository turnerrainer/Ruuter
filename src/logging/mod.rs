//! Structured-logging helpers used by the router, step engine, and
//! HTTP step. Everything user-facing about Ruuter's log surface —
//! semantic-convention fields, redaction, body caps, error-chain
//! rendering — lives here.
//!
//! Reference: `book/src/logging/` (operator-facing chapter).

pub mod fmt;
pub mod redact;

use crate::config::LoggingConfig;

/// Convert an elapsed `Duration` to milliseconds at microsecond
/// precision. Renders cleanly (`0.051`) instead of the f64-artefact
/// tail (`0.05121800000000001`) that `as_secs_f64() * 1000.0`
/// produces. Precision is 0.001 ms = 1 µs, plenty for anything a
/// log reader cares about; JSON consumers still receive a numeric
/// value.
pub fn duration_ms(d: std::time::Duration) -> f64 {
    d.as_micros() as f64 / 1000.0
}

/// Cap used by [`preview_body_for_log`] for step-line body previews.
/// Deliberately much smaller than `LoggingConfig::max_body_bytes`
/// (default 2 KiB): step-line previews sit inside a one-line-per-
/// event terminal format, so a payload preview that spills onto a
/// second line defeats the readability goal. 80 chars is roughly the
/// width of a git commit subject line — enough to identify shape,
/// short enough to keep the line unwrapped.
pub const STEP_PREVIEW_MAX_BYTES: usize = 80;

/// Render a JSON value as a compact single-line preview for a step's
/// `attrs` field. Redacts per `cfg.redact_body_fields` and caps at
/// [`STEP_PREVIEW_MAX_BYTES`]. Returns `None` for `null`/absent
/// values (so the caller can skip the field entirely rather than
/// emit `return.body=null` noise). Unlike [`render_body_for_log`],
/// which is sized for the outbound-body DEBUG line, this helper
/// exists specifically so a step-line preview fits on one terminal
/// row.
pub fn preview_body_for_log(
    value: Option<&serde_json::Value>,
    cfg: &LoggingConfig,
) -> Option<String> {
    let v = value?;
    if v.is_null() {
        return None;
    }
    let redacted = redact::redact_json(v, &cfg.redact_body_fields);
    let s = serde_json::to_string(&redacted).ok()?;
    Some(cap_and_sanitize(&s, STEP_PREVIEW_MAX_BYTES))
}

/// Strip CR / LF from a value about to enter a log field. Prevents
/// log-line splicing when an attacker-controlled header or body
/// field carries a newline. Cheap on the common case (no newline →
/// same allocation).
pub fn sanitize_log_value(s: &str) -> String {
    if !s.contains('\n') && !s.contains('\r') {
        return s.to_string();
    }
    s.replace(['\n', '\r'], " ")
}

/// Format an error's `source()` chain as ` -> caused by: <msg>` links,
/// bounded to 5 hops so a runaway cause chain can't fill a log line.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = String::new();
    let mut src = err.source();
    let mut hops = 0;
    while let Some(e) = src {
        if hops >= 5 {
            out.push_str(" -> caused by: ...");
            break;
        }
        out.push_str(" -> caused by: ");
        out.push_str(&sanitize_log_value(&e.to_string()));
        src = e.source();
        hops += 1;
    }
    out
}

/// Extract the 32-hex trace id from a W3C `traceparent` value, or
/// `None` if the input isn't well-formed. Cheap; used by the router
/// to decorate every request-scoped log line with `trace_id=…`.
pub fn trace_id_from_traceparent(tp: &str) -> Option<&str> {
    let parts: Vec<&str> = tp.splitn(4, '-').collect();
    if parts.len() == 4 && parts[1].len() == 32 {
        Some(parts[1])
    } else {
        None
    }
}

/// Render a JSON value for a log line, redacting configured field
/// names at any depth and capping the serialised length. Returns
/// `"-"` when nothing to render (matches Java's `-` placeholder).
pub fn render_body_for_log(value: Option<&serde_json::Value>, cfg: &LoggingConfig) -> String {
    match value {
        Some(v) => {
            let redacted = redact::redact_json(v, &cfg.redact_body_fields);
            let s = serde_json::to_string(&redacted).unwrap_or_else(|_| "-".to_string());
            cap_and_sanitize(&s, cfg.max_body_bytes)
        }
        None => "-".to_string(),
    }
}

/// Cap the string at `max_bytes` (grapheme-safe: cuts at char
/// boundary) and sanitize CR/LF. Appends `…` when truncated.
pub fn cap_and_sanitize(s: &str, max_bytes: usize) -> String {
    let sanitized = sanitize_log_value(s);
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    let mut end = max_bytes;
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    out.push_str(&sanitized[..end]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_stripped_from_log_value() {
        assert_eq!(sanitize_log_value("a\nb\rc"), "a b c");
        assert_eq!(sanitize_log_value("plain"), "plain");
    }

    #[test]
    fn trace_id_extract() {
        assert_eq!(
            trace_id_from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(trace_id_from_traceparent("garbage"), None);
    }

    #[test]
    fn error_chain_bounded() {
        use std::error::Error;
        use std::fmt;
        #[derive(Debug)]
        struct E {
            msg: &'static str,
            source: Option<Box<E>>,
        }
        impl fmt::Display for E {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", self.msg)
            }
        }
        impl Error for E {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                self.source.as_deref().map(|e| e as &(dyn Error + 'static))
            }
        }
        let deep = (0..10).fold(
            E {
                msg: "leaf",
                source: None,
            },
            |acc, _| E {
                msg: "wrap",
                source: Some(Box::new(acc)),
            },
        );
        let chain = error_chain(&deep);
        assert!(chain.contains("caused by"));
        assert!(chain.ends_with("caused by: ..."));
    }

    #[test]
    fn cap_body_ok_shorter() {
        assert_eq!(cap_and_sanitize("hi", 100), "hi");
    }

    #[test]
    fn cap_body_truncates() {
        let out = cap_and_sanitize(&"x".repeat(50), 10);
        assert_eq!(out.len(), 10 + '…'.len_utf8());
        assert!(out.ends_with('…'));
    }
}

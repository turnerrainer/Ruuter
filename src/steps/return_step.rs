use crate::config::LoggingConfig;
use crate::context::ExecutionContext;
use crate::logging::preview_body_for_log;
use crate::scripting::ScriptEngine;
use crate::steps::{ReturnStep, StepExecutor, StepLogExtras, StepResult};
use crate::{Result, RuuterError};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Java-parity `Set-Cookie` defaults. When the DSL emits a
/// `Set-Cookie` header as a JSON object (`{ name: "session", value:
/// "abc" }`) — or any object shape — these attributes are added if
/// not already present. Matches
/// `ReturnStep.addDefaultCookies` in the Java Ruuter.
///
/// See audit finding 05.
const SET_COOKIE_HEADER: &str = "Set-Cookie";

pub struct ReturnStepExecutor {
    step: ReturnStep,
    script_engine: ScriptEngine,
    /// Framework-wide logging config, threaded in so the step-line
    /// return-body preview honours `redact_body_fields` (a project
    /// that extends the redact list must see the extension respected
    /// in every log surface, not just the outbound-body DEBUG line).
    /// Falls back to defaults when the engine didn't supply one — the
    /// framework's default `redact_body_fields` covers password / token
    /// / secret / api_key, so an unconfigured executor is still safe
    /// against the common cases.
    logging: Arc<LoggingConfig>,
}

impl ReturnStepExecutor {
    pub fn new(step: ReturnStep) -> Self {
        Self::with_logging(step, Arc::new(LoggingConfig::default()))
    }

    pub fn with_logging(step: ReturnStep, logging: Arc<LoggingConfig>) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
            logging,
        }
    }
}

impl StepExecutor for ReturnStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let return_value = self
            .script_engine
            .evaluate(&self.step.return_value, context)?;

        // status: literal u16 or script expression that resolves to a number
        let status = if let Some(s) = &self.step.status {
            let evaluated = self.script_engine.evaluate(s, context)?;
            match evaluated {
                Value::Number(n) => n.as_u64().map(|u| u as u16),
                _ => None,
            }
        } else {
            None
        };

        // Audit finding 05: header values are evaluated RECURSIVELY
        // via the script engine (matches Java's `evaluateScripts` on
        // Map<String, Object>). Cookie-as-map values then get
        // Java-parity defaults added (HttpOnly, Secure, Path=/,
        // Max-Age=28800) before being serialised.
        // Issue #25 — headers can be either a YAML mapping (values
        // may each carry `${…}`) or a single `${expr}` string that
        // evaluates to an object. Normalise to a mapping first, then
        // render each value (with Set-Cookie's Java-parity defaults).
        let source_map: Option<serde_json::Map<String, Value>> = match &self.step.headers {
            None => None,
            Some(Value::Object(m)) => Some(m.clone()),
            Some(Value::String(_)) => {
                let evaluated = self
                    .script_engine
                    .evaluate(self.step.headers.as_ref().unwrap(), context)?;
                match evaluated {
                    Value::Object(m) => Some(m),
                    Value::Null => None,
                    other => {
                        return Err(RuuterError::DslExecution {
                            step: "return".into(),
                            message: format!(
                                "return step `headers`: expression must evaluate to an object, got {}",
                                match other {
                                    Value::Bool(_) => "boolean",
                                    Value::Number(_) => "number",
                                    Value::String(_) => "string",
                                    Value::Array(_) => "array",
                                    _ => "other",
                                }
                            ),
                        });
                    }
                }
            }
            Some(other) => {
                return Err(RuuterError::DslExecution {
                    step: "return".into(),
                    message: format!(
                        "return step `headers`: expected a mapping or `${{expr}}` string, got {:?}",
                        other
                    ),
                });
            }
        };

        let headers = if let Some(h) = source_map {
            let mut out = HashMap::with_capacity(h.len());
            for (k, v) in h {
                // Recursive script eval — nested `${…}` inside map /
                // array values resolves against context.
                let evaluated = self.script_engine.evaluate(&v, context)?;

                let header_value = if k.eq_ignore_ascii_case(SET_COOKIE_HEADER) {
                    render_set_cookie(&evaluated)
                } else {
                    match evaluated {
                        Value::String(s) => s,
                        other => other.to_string(),
                    }
                };
                out.insert(k, header_value);
            }
            Some(out)
        } else {
            None
        };

        // Audit finding 05/12: Java default is wrapper = true. We
        // stash the DSL's wrapper preference into StepResult so the
        // router (which is the response-serialisation layer) can
        // honour it. Unset in DSL = None here = router treats as
        // default-true (Java parity).
        let mut extras = StepLogExtras::new().push("status", status.unwrap_or(200));
        if let Some(w) = self.step.wrapper {
            extras = extras.push("wrapper", w);
        }
        // Issue: the return step was the only "answer" step that
        // showed just status, not the returned content. Add a
        // capped + redacted single-line preview so a reader sees
        // WHAT the DSL returned without cross-referencing the
        // response body or another log line. Skipped when the value
        // is null so we don't get `return.body=null` noise on
        // control-flow-only returns.
        if let Some(preview) = preview_body_for_log(Some(&return_value), &self.logging) {
            extras = extras.push_preformatted("body", preview);
        }
        Ok(StepResult {
            should_return: true,
            return_value: Some(return_value),
            return_status: status,
            return_headers: headers,
            return_wrapper: self.step.wrapper,
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

/// Render a `Set-Cookie` header value. When `value` is a string,
/// pass through as-is. When it's a JSON object, serialise as
/// `key=value; key=value; …` with Java-parity defaults filled in
/// (Path=/, HttpOnly, Secure, Max-Age=28800) unless the DSL already
/// specified them.
///
/// The map shape mirrors Java's cookie-as-Object payload: any key
/// whose value is `true` renders as a bare flag (`HttpOnly;`); any
/// key whose value is a string renders as `key=value; `; other
/// types are stringified.
fn render_set_cookie(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            // Preserve insertion order (serde_json::Map is an
            // IndexMap when the `preserve_order` feature is on — we
            // haven't enabled it explicitly, so keys are sorted
            // alphabetically. That's fine for cookies because
            // `Set-Cookie` attribute order doesn't affect semantics.
            let mut out = serde_json::Map::with_capacity(map.len() + 4);
            for (k, v) in map {
                out.insert(k.clone(), v.clone());
            }
            add_default(&mut out, "Path", Value::String("/".into()));
            add_default(&mut out, "HttpOnly", Value::Bool(true));
            add_default(&mut out, "Secure", Value::Bool(true));
            add_default(
                &mut out,
                "Max-Age",
                Value::Number(serde_json::Number::from(28_800u64)),
            );

            let mut parts: Vec<String> = Vec::with_capacity(out.len());
            for (k, v) in &out {
                match v {
                    Value::Bool(true) => parts.push(k.to_string()),
                    Value::Bool(false) => {
                        // false means "don't set" — drop the flag
                    }
                    Value::String(s) => parts.push(format!("{}={}", k, s)),
                    other => parts.push(format!("{}={}", k, other)),
                }
            }
            parts.join("; ")
        }
        other => other.to_string(),
    }
}

fn add_default(map: &mut serde_json::Map<String, Value>, key: &str, default: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), default);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_set_cookie_passes_through_verbatim() {
        assert_eq!(
            render_set_cookie(&Value::String("session=abc".into())),
            "session=abc"
        );
    }

    #[test]
    fn object_set_cookie_adds_java_parity_defaults() {
        let out = render_set_cookie(&json!({ "session": "abc" }));
        assert!(
            out.contains("session=abc"),
            "kept the DSL-authored pair: {out}"
        );
        assert!(out.contains("HttpOnly"), "added HttpOnly: {out}");
        assert!(out.contains("Secure"), "added Secure: {out}");
        assert!(out.contains("Path=/"), "added Path=/: {out}");
        assert!(out.contains("Max-Age=28800"), "added Max-Age=28800: {out}");
    }

    #[test]
    fn object_set_cookie_does_not_override_dsl_authored_max_age() {
        let out = render_set_cookie(&json!({ "session": "abc", "Max-Age": 60 }));
        assert!(out.contains("Max-Age=60"), "kept DSL Max-Age: {out}");
        assert!(!out.contains("Max-Age=28800"));
    }

    #[test]
    fn object_set_cookie_drops_explicit_false_flags() {
        // If the DSL explicitly sets HttpOnly: false, the flag is
        // omitted (Java: entry produces empty string for false).
        let out = render_set_cookie(&json!({ "session": "abc", "HttpOnly": false }));
        assert!(!out.contains("HttpOnly"), "dropped HttpOnly: false: {out}");
        assert!(out.contains("session=abc"));
    }
}

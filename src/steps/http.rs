use crate::context::ExecutionContext;
use crate::http_client::HttpClient;
use crate::logging::{cap_and_sanitize, redact, render_body_for_log};
use crate::scripting::ScriptEngine;
use crate::steps::engine::StepEngine;
use crate::steps::{HttpStep, StepExecutor, StepResult};
use crate::{Result, RuuterError};
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

pub struct HttpStepExecutor {
    step: HttpStep,
    script_engine: ScriptEngine,
    http_client: HttpClient,
    /// Audit finding 13 — carries the StepEngine so, on an upstream
    /// error without a local `error:` step, we can invoke the
    /// configured default_dsl_in_case_of_exception. Optional so the
    /// pre-existing single-arg constructor still works.
    engine: Option<StepEngine>,
}

impl HttpStepExecutor {
    pub fn new(step: HttpStep, http_client: HttpClient) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
            http_client,
            engine: None,
        }
    }

    /// Constructor variant that gives the step access to the engine
    /// so it can invoke the default exception DSL on failure.
    pub fn with_engine(step: HttpStep, http_client: HttpClient, engine: StepEngine) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
            http_client,
            engine: Some(engine),
        }
    }
}

impl StepExecutor for HttpStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let method = self.parse_method()?;

        let url = self
            .script_engine
            .evaluate(&Value::String(self.step.args.url.clone()), context)?;

        let body = if let Some(b) = &self.step.args.body {
            Some(self.script_engine.evaluate(b, context)?)
        } else {
            None
        };

        // Issue #25 — accept either a YAML mapping (per-key values,
        // each with `${…}` interpolation) or a whole-map `${expr}`
        // string that evaluates to an object at runtime. Same for
        // headers below.
        let query = evaluate_map_arg(
            self.step.args.query.as_ref(),
            &self.script_engine,
            context,
            "http",
            "query",
        )?;
        let mut headers = evaluate_map_arg(
            self.step.args.headers.as_ref(),
            &self.script_engine,
            context,
            "http",
            "headers",
        )?
        .unwrap_or_default();

        // Auto-forward traceparent unless the DSL already set one — every
        // Buerostack component participates in W3C tracecontext by default
        // (PATTERNS.md §4).
        if !headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("traceparent"))
        {
            if let Some(tp) = context.traceparent() {
                headers.insert("traceparent".to_string(), Value::String(tp.to_string()));
            }
        }
        let headers = if headers.is_empty() {
            None
        } else {
            Some(headers)
        };

        let timeout = self.step.timeout.map(Duration::from_millis);

        // Per-step logging knobs: DEBUG lines with redacted +
        // capped request / response bodies, gated by
        // `logging.display_request_content` /
        // `display_response_content`. Java-parity fields
        // (Java Ruuter emitted these into MDC).
        let logging = self.engine.as_ref().map(|e| e.logging());
        let url_str = url.as_str().unwrap_or("").to_string();

        if let Some(cfg) = logging.as_deref() {
            if cfg.display_request_content {
                let body_rendered = render_body_for_log(body.as_ref(), cfg);
                let headers_rendered = headers
                    .as_ref()
                    .map(|h| {
                        let sh: HashMap<String, String> = h
                            .iter()
                            .map(|(k, v)| (k.clone(), v.to_string()))
                            .collect();
                        redact::redact_headers(&sh, &cfg.redact_headers)
                    })
                    .unwrap_or_default();
                tracing::debug!(
                    http.request.method = %method,
                    url.full = %cap_and_sanitize(&url_str, 512),
                    http.request.body = %body_rendered,
                    http.request.headers = ?headers_rendered,
                    "outbound http request"
                );
            }
        }

        // Audit finding 11 — thread the DSL's `content_type:` into the
        // transport so it can pick JSON / plaintext / formdata /
        // multipart / dynamicBody / json_override behaviour.
        let response = self
            .http_client
            .request_with_ct(
                method.clone(),
                url.as_str().unwrap_or(""),
                body.as_ref(),
                query.as_ref(),
                headers.as_ref(),
                timeout,
                self.step.args.content_type.as_deref(),
            )
            .await?;

        if let Some(cfg) = logging.as_deref() {
            if cfg.display_response_content {
                let body_rendered = render_body_for_log(response.body.as_ref(), cfg);
                let headers_rendered =
                    redact::redact_headers(&response.headers, &cfg.redact_headers);
                tracing::debug!(
                    http.request.method = %method,
                    url.full = %cap_and_sanitize(&url_str, 512),
                    http.response.status_code = response.status,
                    http.response.body = %body_rendered,
                    http.response.headers = ?headers_rendered,
                    "upstream http response"
                );
            }
        }

        // Bind the result BEFORE the allow-list check so the
        // `error:` step (and downstream steps in general) can read
        // the upstream status / body via `${resultName.response.*}`.
        // Matches Java: DefaultHttpDsl reads the failed response's
        // status / body from the same context slot.
        if let Some(result_name) = &self.step.result {
            let result_value = json!({
                "response": {
                    "status": response.status,
                    "body": response.body,
                    "headers": response.headers,
                }
            });
            context.set_variable(result_name.clone(), result_value);
        }

        // Audit finding 04: honour the DSL's `error:` field on
        // non-allowed status. Java's HttpStep:
        //   if (!isAllowedHttpStatusCode(...)) {
        //     if (getOnErrorStep() != null) { setNextStepName(onErrorStep); }
        //     else { throw new IllegalArgumentException(); }
        //   }
        // We mirror that: on non-allowed status, route to `error:`
        // if set; otherwise propagate. When the step has both an
        // `error:` and a `next:`, `error:` wins on failure.
        if !self.http_client.is_status_allowed(response.status) {
            if let Some(err_step) = &self.step.error {
                return Ok(StepResult {
                    next_step: Some(err_step.clone()),
                    ..StepResult::new()
                });
            }
            // Audit finding 13: fallback DSL. Java's
            // `defaultDslInCaseOfException` runs the configured
            // recovery DSL before the throw. We invoke it here
            // (fire-and-await) so the fallback can log / notify /
            // enqueue-for-retry / return-through-the-parent. The
            // fallback's return value is DISCARDED — we still
            // propagate the upstream error to the caller (matches
            // Java which throws IllegalArgumentException after
            // executing the default DSL). If the fallback itself
            // errors, log it and continue with the original error.
            if let Some(engine) = &self.engine {
                if let Some(cfg) = engine.default_exception_dsl().cloned() {
                    let failed_id = context.trace_id();
                    if let Err(e) = engine
                        .invoke_default_exception_dsl(
                            &cfg,
                            response.status,
                            response.body.as_ref(),
                            failed_id.as_deref(),
                            context,
                        )
                        .await
                    {
                        tracing::error!(
                            "default exception DSL '{}' failed: {} (original upstream status was {})",
                            cfg.dsl, e, response.status
                        );
                    }
                }
            }
            return Err(RuuterError::HttpRequest(format!(
                "upstream status {} not in http_codes_allow_list",
                response.status
            )));
        }

        Ok(StepResult {
            next_step: self.step.next.clone(),
            ..StepResult::new()
        })
    }
}

/// Issue #25 — evaluate a step-arg that YAML can express two ways:
///
/// 1. **YAML mapping** — each value is evaluated per-key (each may
///    itself contain `${…}` expressions). This is the traditional
///    shape.
/// 2. **A `${expr}` string** — evaluated once; the result must be
///    a JSON object, which is then flattened into the map.
///
/// Any other JSON shape (array, scalar, non-object) is a hard
/// runtime error naming the offending arg — DSL authors get a
/// clear diagnostic instead of a silent broken request.
///
/// `arg_name` is the field name (`"headers"` / `"query"`) so
/// error messages point at the right place.
pub(crate) fn evaluate_map_arg(
    arg: Option<&Value>,
    script_engine: &ScriptEngine,
    context: &ExecutionContext,
    step_name: &str,
    arg_name: &str,
) -> Result<Option<HashMap<String, Value>>> {
    let Some(v) = arg else {
        return Ok(None);
    };
    match v {
        Value::Object(map) => {
            let mut out = HashMap::with_capacity(map.len());
            for (k, val) in map {
                out.insert(k.clone(), script_engine.evaluate(val, context)?);
            }
            Ok(Some(out))
        }
        Value::String(_) => {
            // Whole-map expression form. Evaluate once; require object.
            let evaluated = script_engine.evaluate(v, context)?;
            match evaluated {
                Value::Object(map) => Ok(Some(map.into_iter().collect())),
                Value::Null => Ok(None),
                other => Err(RuuterError::DslExecution {
                    step: step_name.into(),
                    message: format!(
                        "{} step arg `{}`: expression must evaluate to an object, got {}",
                        step_name,
                        arg_name,
                        json_kind(&other)
                    ),
                }),
            }
        }
        other => Err(RuuterError::DslExecution {
            step: step_name.into(),
            message: format!(
                "{} step arg `{}`: expected a YAML mapping or `${{expr}}` string, got {}",
                step_name,
                arg_name,
                json_kind(other)
            ),
        }),
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl HttpStepExecutor {
    fn parse_method(&self) -> Result<Method> {
        match self.step.call.as_str() {
            "http.get" => Ok(Method::GET),
            "http.post" => Ok(Method::POST),
            "http.put" => Ok(Method::PUT),
            "http.patch" => Ok(Method::PATCH),
            "http.delete" => Ok(Method::DELETE),
            _ => Err(RuuterError::InvalidStep(format!(
                "Unknown HTTP method: {}",
                self.step.call
            ))),
        }
    }
}

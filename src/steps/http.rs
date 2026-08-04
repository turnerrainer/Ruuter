use crate::context::ExecutionContext;
use crate::http_client::HttpClient;
use crate::scripting::ScriptEngine;
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
}

impl HttpStepExecutor {
    pub fn new(step: HttpStep, http_client: HttpClient) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
            http_client,
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

        let query = if let Some(q) = &self.step.args.query {
            let mut evaluated = HashMap::new();
            for (k, v) in q {
                evaluated.insert(k.clone(), self.script_engine.evaluate(v, context)?);
            }
            Some(evaluated)
        } else {
            None
        };

        let mut headers = if let Some(h) = &self.step.args.headers {
            let mut evaluated = HashMap::new();
            for (k, v) in h {
                evaluated.insert(k.clone(), self.script_engine.evaluate(v, context)?);
            }
            evaluated
        } else {
            HashMap::new()
        };

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

        let response = self
            .http_client
            .request(
                method,
                url.as_str().unwrap_or(""),
                body.as_ref(),
                query.as_ref(),
                headers.as_ref(),
                timeout,
            )
            .await?;

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

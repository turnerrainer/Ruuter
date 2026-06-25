use crate::context::ExecutionContext;
use crate::http_client::{HttpClient, HttpResponse};
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

        let url = self.script_engine.evaluate(
            &Value::String(self.step.args.url.clone()),
            context
        )?;

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

        let headers = if let Some(h) = &self.step.args.headers {
            let mut evaluated = HashMap::new();
            for (k, v) in h {
                evaluated.insert(k.clone(), self.script_engine.evaluate(v, context)?);
            }
            Some(evaluated)
        } else {
            None
        };

        let timeout = self.step.timeout.map(Duration::from_millis);

        let response = self.http_client.request(
            method,
            url.as_str().unwrap_or(""),
            body.as_ref(),
            query.as_ref(),
            headers.as_ref(),
            timeout,
        ).await?;

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

        Ok(StepResult::with_next(
            self.step.next.clone().unwrap_or_else(|| "end".to_string())
        ))
    }
}

impl HttpStepExecutor {
    fn parse_method(&self) -> Result<Method> {
        match self.step.call.as_str() {
            "http.get" => Ok(Method::GET),
            "http.post" => Ok(Method::POST),
            "http.put" => Ok(Method::PUT),
            "http.delete" => Ok(Method::DELETE),
            _ => Err(RuuterError::InvalidStep(format!("Unknown HTTP method: {}", self.step.call))),
        }
    }
}

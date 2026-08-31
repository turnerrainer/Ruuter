//! Java-parity `call: reflect.mock` step (audit finding 09).
//!
//! Runs no HTTP request; binds a synthetic result under `result:` so
//! downstream steps see the same `{ response: { status, body, headers } }`
//! shape as a real `http.<verb>` call. Useful for DSL unit tests that
//! want to isolate branching / mapping logic from an actual upstream.
//!
//! Mirrors Java's `HttpMockStep`. The `request:` field on args is
//! optional — when present, it is echoed under `.request` on the
//! synthetic result for DSLs that want to assert on "what would have
//! been sent"; when absent, the request half is omitted (Java behaviour).

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::{HttpMockStep, StepExecutor, StepLogExtras, StepResult};
use crate::Result;
use serde_json::json;

pub struct HttpMockStepExecutor {
    step: HttpMockStep,
    script_engine: ScriptEngine,
}

impl HttpMockStepExecutor {
    pub fn new(step: HttpMockStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for HttpMockStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        // Evaluate response body against context so mocks can reflect
        // DSL variables (matches Java's evaluateScripts pattern).
        let body = self
            .script_engine
            .evaluate(&self.step.args.response, context)?;
        let status = self.step.args.status.unwrap_or(200);

        if let Some(result_name) = &self.step.result {
            let mut bound = json!({
                "response": {
                    "status": status,
                    "body": body,
                    "headers": {},
                }
            });
            if let Some(req) = &self.step.args.request {
                let req_evaluated = self.script_engine.evaluate(req, context)?;
                bound["request"] = req_evaluated;
            }
            context.set_variable(result_name.clone(), bound);
        }

        // Finding 03 fix: return `next` as-is (None → engine falls
        // through to source-order next). Never force `"end"`.
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new().push("status", status),
            ..StepResult::new()
        })
    }
}

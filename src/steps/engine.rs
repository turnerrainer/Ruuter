//! Shared step-execution engine. Used by both the HTTP router
//! (`DslRouter`) and the event-trigger dispatcher (`TriggerDispatcher`)
//! so DSL semantics are identical regardless of where the request /
//! event originated.

use crate::context::ExecutionContext;
use crate::dsl::loader::HttpDsls;
use crate::dsl::Dsl;
use crate::http_client::HttpClient;
use crate::scripting::ExpressionRegistry;
use crate::steps::single_flight::Registry as SingleFlightRegistry;
use crate::steps::{
    assign, http, iterate, log, return_step, single_flight, state, switch, template, ws_send,
    DslStep, StepExecutor,
};
use crate::ws::WsRegistry;
use crate::{Result, RuuterError};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct StepEngine {
    http_client: HttpClient,
    ws_registry: WsRegistry,
    max_iterations: u32,
    /// Shared handle to the loaded HTTP DSL tree. Used by the
    /// template step to look up callee DSLs at runtime. `None` = template
    /// step still works as a placeholder (logs a warning and advances).
    dsls: Option<Arc<HttpDsls>>,
    /// Task 042 — process-wide single_flight coalescing registry.
    /// Shared by cloning (Arc inside). One StepEngine → one map;
    /// every single_flight step in every DSL keys into it. Since
    /// keys are DSL-computed (not framework-generated), collisions
    /// across unrelated DSLs are the operator's responsibility to
    /// avoid by namespacing keys (e.g. `"cache-warmer:${id}"`).
    single_flight: SingleFlightRegistry,
    /// Task 045 — pre-parsed expression registry (built once at
    /// boot from the loaded DSL tree). Empty by default; router
    /// / dispatcher set it via `with_expr_registry()`. Passed to
    /// every `ExecutionContext` this engine constructs during
    /// step dispatch.
    expr_registry: ExpressionRegistry,
}

#[derive(Debug)]
pub struct DslExecutionResult {
    pub value: Option<Value>,
    pub status: u16,
    pub headers: HashMap<String, String>,
}

impl StepEngine {
    pub fn new(http_client: HttpClient) -> Self {
        Self {
            http_client,
            ws_registry: WsRegistry::new(),
            // Lifted from the original 100 — that was a footgun for any
            // DSL with non-trivial branching. `iterate` has its own
            // per-step bound; this is a sanity cap on top-level step
            // transitions only.
            max_iterations: 10_000,
            dsls: None,
            single_flight: SingleFlightRegistry::new(),
            expr_registry: ExpressionRegistry::default(),
        }
    }

    /// Task 045 — attach the pre-parsed expression registry the
    /// scripting backend will consult. Populated from the loaded
    /// DSL tree at boot; empty otherwise (backends fall back to
    /// per-eval compilation without missing behaviour).
    pub fn with_expr_registry(mut self, registry: ExpressionRegistry) -> Self {
        self.expr_registry = registry;
        self
    }

    pub fn expr_registry(&self) -> &ExpressionRegistry {
        &self.expr_registry
    }

    /// Exposed for tests + operator diagnostics — returns a handle
    /// to the same underlying registry every clone of this engine
    /// shares.
    pub fn single_flight_registry(&self) -> &SingleFlightRegistry {
        &self.single_flight
    }

    /// Attach a shared handle to the loaded HTTP DSL tree so the
    /// template step can resolve callee DSLs at runtime.
    pub fn with_dsls(mut self, dsls: Arc<HttpDsls>) -> Self {
        self.dsls = Some(dsls);
        self
    }

    pub fn dsls(&self) -> Option<&Arc<HttpDsls>> {
        self.dsls.as_ref()
    }

    /// Attach a shared WS registry. The engine needs this so the
    /// `ws_send` step can resolve connection ids → writer channels.
    pub fn with_ws_registry(mut self, registry: WsRegistry) -> Self {
        self.ws_registry = registry;
        self
    }

    pub fn ws_registry(&self) -> &WsRegistry {
        &self.ws_registry
    }

    pub fn with_max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = n;
        self
    }

    pub async fn run(&self, dsl: &Dsl, context: &ExecutionContext) -> Result<DslExecutionResult> {
        let step_names = dsl.step_names();
        let mut current_step_idx = 0;
        let mut budget = self.max_iterations;

        while current_step_idx < step_names.len() && budget > 0 {
            budget -= 1;

            let step_name = &step_names[current_step_idx];
            let step = dsl.get_step(step_name)
                .ok_or_else(|| RuuterError::InvalidStep(format!("Step not found: {}", step_name)))?;

            let result = self.execute_single_step(step, context).await?;

            if result.should_return {
                return Ok(DslExecutionResult {
                    value: result.return_value,
                    status: result.return_status.unwrap_or(200),
                    headers: result.return_headers.unwrap_or_default(),
                });
            }

            if let Some(next) = result.next_step {
                if next == "end" {
                    break;
                }
                if let Some(idx) = step_names.iter().position(|n| n == &next) {
                    current_step_idx = idx;
                } else {
                    break;
                }
            } else {
                current_step_idx += 1;
            }
        }

        Ok(DslExecutionResult {
            value: None,
            status: 200,
            headers: HashMap::new(),
        })
    }

    /// Dispatch a single step. Exposed (`pub`) so composite steps like
    /// `iterate` can drive their body sub-pipelines through the same
    /// engine without duplicating the dispatch table.
    ///
    /// Returns a boxed future — necessary because `Iterate` calls back
    /// into this function, creating a recursive async cycle that the
    /// compiler cannot infer a fixed opaque type for.
    pub fn execute_single_step<'a>(
        &'a self,
        step: &'a DslStep,
        context: &'a ExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::steps::StepResult>> + Send + 'a>> {
        Box::pin(self.execute_single_step_impl(step, context))
    }

    async fn execute_single_step_impl(&self, step: &DslStep, context: &ExecutionContext) -> Result<crate::steps::StepResult> {
        match step {
            DslStep::Assign(s) => {
                assign::AssignStepExecutor::new(s.clone()).execute(context).await
            }
            DslStep::Return(s) => {
                return_step::ReturnStepExecutor::new(s.clone()).execute(context).await
            }
            DslStep::Http(s) => {
                http::HttpStepExecutor::new(s.clone(), self.http_client.clone()).execute(context).await
            }
            DslStep::Switch(s) => {
                switch::SwitchStepExecutor::new(s.clone()).execute(context).await
            }
            DslStep::Log(s) => {
                log::LogStepExecutor::new(s.clone()).execute(context).await
            }
            DslStep::Template(s) => {
                template::TemplateStepExecutor::new(s.clone(), self.clone()).execute(context).await
            }
            DslStep::State(s) => {
                state::StateStepExecutor::new(s.clone()).execute(context).await
            }
            DslStep::Iterate(s) => {
                iterate::IterateStepExecutor::new(s.clone(), self.clone()).execute(context).await
            }
            DslStep::WsSend(s) => {
                ws_send::WsSendStepExecutor::new(s.clone(), self.ws_registry.clone()).execute(context).await
            }
            DslStep::SingleFlight(s) => {
                single_flight::SingleFlightStepExecutor::new(
                    s.clone(),
                    self.clone(),
                    self.single_flight.clone(),
                )
                .execute(context)
                .await
            }
            DslStep::Declaration(_) => {
                // Declaration is DSL metadata (OpenAPI hints,
                // override_ancestors flag, …). Treat as a no-op step —
                // fall through to the next step in source order rather
                // than terminating the run.
                Ok(crate::steps::StepResult::new())
            }
        }
    }
}

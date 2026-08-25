//! Shared step-execution engine. Used by both the HTTP router
//! (`DslRouter`) and the event-trigger dispatcher (`TriggerDispatcher`)
//! so DSL semantics are identical regardless of where the request /
//! event originated.

use crate::config::LoggingConfig;
use crate::context::ExecutionContext;
use crate::dsl::loader::{HttpDsls, SharedHttpDsls};
use crate::dsl::Dsl;
use crate::http_client::HttpClient;
use crate::logging::error_chain;
use crate::scripting::ExpressionRegistry;
use crate::steps::single_flight::Registry as SingleFlightRegistry;
use crate::steps::{
    assign, http, http_mock, iterate, log, return_step, single_flight, state, switch, template,
    ws_send, DslStep, StepExecutor,
};
use crate::ws::WsRegistry;
use crate::{Result, RuuterError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, warn};

/// Wired by the framework (usually the router) so the engine can
/// honour Java-parity `reloadDsl: true` step field (audit finding 01).
/// A step-triggered reload calls this on the engine's handle; the
/// handle re-runs the DSL loader and republishes the tree — same
/// mechanism the filesystem watcher uses.
#[async_trait]
pub trait ReloadHandler: Send + Sync {
    async fn trigger_reload(&self);
}

#[derive(Clone)]
pub struct StepEngine {
    http_client: HttpClient,
    ws_registry: WsRegistry,
    max_iterations: u32,
    /// Shared, atomically-swappable handle to the loaded HTTP DSL
    /// tree. Used by the template step to resolve callee DSLs at
    /// runtime. `None` = template step still works as a placeholder
    /// (logs a warning and advances). When hot-reload is enabled, the
    /// router and this handle point at the same `ArcSwap`, so a
    /// single publish on the router is visible here immediately.
    dsls: Option<SharedHttpDsls>,
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
    /// Audit finding 01 — reloadDsl step handler. Empty OnceCell
    /// = requests to reload are logged and no-op'd (matches "not
    /// enabled in configuration" branch in Java). Populated =
    /// trigger_reload is called after every step that carries
    /// `reload_dsl: true`. Wrapped in Arc<OnceCell> because the
    /// engine is built BEFORE the router (which the handler needs
    /// to publish onto); main.rs sets the handler post-hoc via
    /// `set_reload_handler`. Every clone of the engine sees the
    /// same shared slot.
    reload_handler: Arc<once_cell::sync::OnceCell<Arc<dyn ReloadHandler>>>,
    /// Audit finding 13 — default exception DSL. When set and an
    /// HTTP step's status is outside `http_codes_allow_list` AND
    /// the step has no local `error:`, HttpStepExecutor asks the
    /// engine to invoke this DSL with an enriched body.
    default_exception_dsl: Option<crate::config::DefaultHttpDslConfig>,
    /// Structured-logging config. Shared clone (Arc inside) so every
    /// engine clone observes the same knobs — flipping `step_timing`
    /// or the redact lists is a config-file change + process restart,
    /// not a per-clone runtime decision.
    logging: Arc<LoggingConfig>,
}

#[derive(Debug)]
pub struct DslExecutionResult {
    pub value: Option<Value>,
    pub status: u16,
    pub headers: HashMap<String, String>,
    /// Audit finding 05/12 — Java-parity: default `true`. When the
    /// DSL's terminating `return:` step sets `wrapper: false`, the
    /// router serialises the body raw; otherwise it wraps in
    /// `{"response": <value>}`. `None` means the DSL never reached
    /// a `return:` step (e.g. loop-cap exhaustion) — router treats
    /// that as no wrapper (matches Java's default response shape
    /// for empty return, which is bare null).
    pub wrapper: Option<bool>,
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
            reload_handler: Arc::new(once_cell::sync::OnceCell::new()),
            default_exception_dsl: None,
            logging: Arc::new(LoggingConfig::default()),
        }
    }

    /// Attach the framework-wide logging config so this engine (and
    /// every step it dispatches) honours `step_timing`, body-content
    /// toggles, redact lists, error-chain rendering, etc. Callers
    /// should pass an `Arc` cloned from `AppConfig.logging`.
    pub fn with_logging(mut self, logging: Arc<LoggingConfig>) -> Self {
        self.logging = logging;
        self
    }

    /// Structured-logging knobs shared with every step. Cheap to
    /// clone — the config lives behind an `Arc`.
    pub fn logging(&self) -> Arc<LoggingConfig> {
        self.logging.clone()
    }

    /// Audit finding 13 — attach the default exception DSL config.
    /// HttpStepExecutor invokes it when an upstream call errors and
    /// the step has no local error: handler.
    pub fn with_default_exception_dsl(
        mut self,
        cfg: crate::config::DefaultHttpDslConfig,
    ) -> Self {
        self.default_exception_dsl = Some(cfg);
        self
    }

    pub fn default_exception_dsl(&self) -> Option<&crate::config::DefaultHttpDslConfig> {
        self.default_exception_dsl.as_ref()
    }

    /// Audit finding 13 — invoke the default exception DSL with an
    /// enriched body. Returns the fallback's `DslExecutionResult`
    /// (or an error if the fallback DSL doesn't exist). Body is
    /// merged: framework-provided (statusCode, responseBody,
    /// failedRequestId) then config-declared body fields (config
    /// values win).
    pub async fn invoke_default_exception_dsl(
        &self,
        cfg: &crate::config::DefaultHttpDslConfig,
        upstream_status: u16,
        upstream_body: Option<&serde_json::Value>,
        failed_request_id: Option<&str>,
        parent_ctx: &ExecutionContext,
    ) -> Result<DslExecutionResult> {
        let dsls_handle = self.dsls.as_ref().ok_or_else(|| {
            RuuterError::InvalidStep(
                "default_dsl_in_case_of_exception invoked but engine has no DSL tree".into(),
            )
        })?;
        let dsls = dsls_handle.load();
        let method = cfg.request_type.to_uppercase();
        let dsl_key = format!("{}/{}", method, cfg.dsl.trim_matches('/'));
        let dsl = dsls
            .get(&cfg.project)
            .and_then(|by_method| by_method.get(&method))
            .and_then(|by_key| by_key.get(&dsl_key))
            .ok_or_else(|| {
                RuuterError::FileNotFound(format!(
                    "default exception DSL not found: {}/{} (project={})",
                    method, dsl_key, cfg.project
                ))
            })?
            .clone();

        // Framework-provided enrichment first, then config body over
        // top (config wins per Java's `body.put(...)` before evaluate).
        let mut body: HashMap<String, Value> = HashMap::new();
        body.insert(
            "statusCode".to_string(),
            Value::Number(upstream_status.into()),
        );
        body.insert(
            "responseBody".to_string(),
            upstream_body.cloned().unwrap_or(Value::Null),
        );
        body.insert(
            "failedRequestId".to_string(),
            Value::String(failed_request_id.unwrap_or("").to_string()),
        );
        for (k, v) in &cfg.body {
            body.insert(k.clone(), v.clone());
        }

        let query: HashMap<String, Value> = cfg.query.clone();
        let headers: HashMap<String, String> = cfg.headers.clone();

        let child_ctx = ExecutionContext::with_state(
            body,
            query,
            headers,
            parent_ctx.request_origin().to_string(),
            cfg.project.clone(),
            parent_ctx.state().clone(),
        )
        .with_expr_registry(self.expr_registry.clone());

        self.run(&dsl, &child_ctx).await
    }

    /// Audit finding 01 — install the reload handler used by
    /// `reload_dsl: true` step field. First-write-wins via OnceCell
    /// so main.rs can wire the router-side handler AFTER building
    /// the router (which internally requires the engine). Without a
    /// handler installed, a step-authored reload logs at ERROR and
    /// is otherwise a no-op (matching Java when
    /// `allow_dsl_reloading` is off).
    pub fn set_reload_handler(&self, handler: Arc<dyn ReloadHandler>) {
        if self.reload_handler.set(handler).is_err() {
            tracing::warn!("StepEngine::set_reload_handler called twice — ignoring second");
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
    ///
    /// Wraps the input `Arc<HttpDsls>` in a fresh `ArcSwap` internally.
    /// Callers that want hot-reload — i.e. a single `ArcSwap` shared
    /// with `DslRouter` — should use `with_dsls_shared` instead.
    pub fn with_dsls(mut self, dsls: Arc<HttpDsls>) -> Self {
        self.dsls = Some(Arc::new(ArcSwap::from(dsls)));
        self
    }

    /// Attach a shared, atomically-swappable handle to the loaded
    /// HTTP DSL tree so the template step can resolve callee DSLs at
    /// runtime. The same handle should also be passed to `DslRouter`
    /// so a single hot-reload publish is visible to both.
    pub fn with_dsls_shared(mut self, dsls: SharedHttpDsls) -> Self {
        self.dsls = Some(dsls);
        self
    }

    pub fn dsls(&self) -> Option<&SharedHttpDsls> {
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

        // Audit finding 08: per-step recursion counter (Java-parity).
        // Cap = min(step.max_recursions, self.max_iterations). When
        // exhausted for a specific step, we ADVANCE past it (matches
        // Java's `executeNextStepOutsideRecursion`) rather than
        // terminating the run.
        let mut recursions: HashMap<String, u32> = HashMap::with_capacity(step_names.len());

        while current_step_idx < step_names.len() && budget > 0 {
            budget -= 1;

            let step_name = &step_names[current_step_idx];
            let step = dsl.get_step(step_name).ok_or_else(|| {
                RuuterError::InvalidStep(format!("Step not found: {}", step_name))
            })?;

            // Per-step recursion cap check BEFORE dispatch. On hit,
            // advance to the next step in source order (bumping past
            // the loop) — same behaviour as Java's
            // executeNextStepOutsideRecursion.
            let step_cap = per_step_cap(step, self.max_iterations);
            let current_count = *recursions.get(step_name).unwrap_or(&0);
            if current_count >= step_cap {
                current_step_idx += 1;
                continue;
            }
            recursions.insert(step_name.clone(), current_count + 1);

            // Audit finding 03: skip/sleep are inherited base fields
            // now enforced by the engine, not per-executor. `skip:
            // true` bypasses the action and falls through in source
            // order; `sleep: <ms>` is honoured before dispatch.
            let base = step.base();
            let skipped = base.and_then(|b| b.skip).unwrap_or(false);
            if let Some(sleep_ms) = base.and_then(|b| b.sleep) {
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
            }

            // Per-step timing / error-chain instrumentation. Wall-
            // clock is captured only when at least one of the two
            // knobs is on — cheap check, avoids a `now()` syscall
            // per step for the default config.
            let want_timing = self.logging.step_timing;
            let want_error_chain =
                self.logging.print_stack_trace || self.logging.meaningful_errors;
            let step_started = if want_timing || want_error_chain {
                Some(Instant::now())
            } else {
                None
            };

            let step_outcome = if skipped {
                // Skipped step still counts as a transition (budget
                // decrement above) but produces no state change and
                // no `next:` directive, so the engine falls through
                // to source-order next.
                Ok(crate::steps::StepResult::new())
            } else {
                self.execute_single_step(step, context).await
            };

            let result = match step_outcome {
                Ok(r) => {
                    if want_timing {
                        let elapsed_ms =
                            step_started.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
                        debug!(
                            dsl.step = %step_name,
                            dsl.step.type = %step.type_name(),
                            duration_ms = elapsed_ms,
                            skipped = skipped,
                            "step completed"
                        );
                    }
                    r
                }
                Err(e) => {
                    // Structured error line with optional cause
                    // chain (`print_stack_trace`) and optional
                    // second WARN line with just the underlying
                    // message (`meaningful_errors`, Java parity).
                    let elapsed_ms = step_started
                        .map(|t| t.elapsed().as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    let chain = if self.logging.print_stack_trace {
                        error_chain(&e)
                    } else {
                        String::new()
                    };
                    error!(
                        dsl.step = %step_name,
                        dsl.step.type = %step.type_name(),
                        duration_ms = elapsed_ms,
                        error = %e,
                        cause_chain = %chain,
                        "step failed"
                    );
                    if self.logging.meaningful_errors {
                        if let Some(src) = std::error::Error::source(&e) {
                            warn!(
                                dsl.step = %step_name,
                                cause = %src,
                                "step failed (underlying cause)"
                            );
                        }
                    }
                    return Err(e);
                }
            };

            if result.should_return {
                return Ok(DslExecutionResult {
                    value: result.return_value,
                    status: result.return_status.unwrap_or(200),
                    headers: result.return_headers.unwrap_or_default(),
                    wrapper: result.return_wrapper,
                });
            }

            // Audit finding 01: after the step's action, if the step
            // requested `reload_dsl: true`, ask the handler to
            // republish. Handler is Some when the framework wired
            // one AND (in the router's implementation) config gate
            // allow_dsl_reloading is true. When wired but gated off
            // the handler itself no-ops and logs.
            if !skipped && base.and_then(|b| b.reload_dsl).unwrap_or(false) {
                match self.reload_handler.get() {
                    Some(handler) => {
                        handler.trigger_reload().await;
                    }
                    None => {
                        error!(
                            "step {:?} requested reload_dsl but no reload handler is wired \
                             (dsl.allow_dsl_reloading may be off)",
                            step_name
                        );
                    }
                }
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

        if budget == 0 {
            warn!("DSL run exceeded global max_iterations cap ({}); returning empty response", self.max_iterations);
        }

        Ok(DslExecutionResult {
            value: None,
            status: 200,
            headers: HashMap::new(),
            wrapper: None,
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
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::steps::StepResult>> + Send + 'a>,
    > {
        Box::pin(self.execute_single_step_impl(step, context))
    }

    async fn execute_single_step_impl(
        &self,
        step: &DslStep,
        context: &ExecutionContext,
    ) -> Result<crate::steps::StepResult> {
        match step {
            DslStep::Assign(s) => {
                assign::AssignStepExecutor::new(s.clone())
                    .execute(context)
                    .await
            }
            DslStep::Return(s) => {
                return_step::ReturnStepExecutor::new(s.clone())
                    .execute(context)
                    .await
            }
            DslStep::Http(s) => {
                // Give the executor the engine handle so it can
                // invoke the default exception DSL on upstream error
                // (audit finding 13).
                http::HttpStepExecutor::with_engine(
                    s.clone(),
                    self.http_client.clone(),
                    self.clone(),
                )
                .execute(context)
                .await
            }
            DslStep::HttpMock(s) => {
                http_mock::HttpMockStepExecutor::new(s.clone())
                    .execute(context)
                    .await
            }
            DslStep::Switch(s) => {
                switch::SwitchStepExecutor::new(s.clone())
                    .execute(context)
                    .await
            }
            DslStep::Log(s) => log::LogStepExecutor::new(s.clone()).execute(context).await,
            DslStep::Template(s) => {
                template::TemplateStepExecutor::new(s.clone(), self.clone())
                    .execute(context)
                    .await
            }
            DslStep::State(s) => {
                state::StateStepExecutor::new(s.clone())
                    .execute(context)
                    .await
            }
            DslStep::Iterate(s) => {
                iterate::IterateStepExecutor::new(s.clone(), self.clone())
                    .execute(context)
                    .await
            }
            DslStep::WsSend(s) => {
                ws_send::WsSendStepExecutor::new(s.clone(), self.ws_registry.clone())
                    .execute(context)
                    .await
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

/// Effective per-step recursion cap: `min(step.max_recursions, global)`.
/// Matches Java's `getStepMaxRecursions`. `Declaration` has no base
/// fields, so returns the global cap.
fn per_step_cap(step: &DslStep, global: u32) -> u32 {
    step.base()
        .and_then(|b| b.max_recursions)
        .map(|per_step| per_step.min(global))
        .unwrap_or(global)
}

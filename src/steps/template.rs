use crate::context::ExecutionContext;
use crate::logging::preview_body_for_log;
use crate::scripting::ScriptEngine;
use crate::state::StateStore;
use crate::steps::engine::StepEngine;
use crate::steps::http::evaluate_map_arg;
use crate::steps::{StepExecutor, StepLogExtras, StepResult, TemplateStep};
use crate::{Result, RuuterError};
use serde_json::Value;
use std::collections::HashMap;

pub struct TemplateStepExecutor {
    step: TemplateStep,
    engine: StepEngine,
    script_engine: ScriptEngine,
}

impl TemplateStepExecutor {
    pub fn new(step: TemplateStep, engine: StepEngine) -> Self {
        Self {
            step,
            engine,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for TemplateStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let dsls_handle = self.engine.dsls().ok_or_else(|| {
            RuuterError::InvalidStep(
                "template step invoked but the engine has no DSL tree \
                 attached — use StepEngine::with_dsls at boot"
                    .into(),
            )
        })?;
        // Snapshot the tree for the duration of this lookup. Hot-reload
        // publishes replace the underlying pointer atomically, so the
        // guard we hold here observes a coherent view even mid-swap.
        let dsls = dsls_handle.load();

        let project = context.project();
        let method = self
            .step
            .request_type
            .clone()
            .unwrap_or_else(|| "GET".to_string())
            .to_uppercase();
        // `template` field is a project-relative path like
        // `templates/user-profile`. The DSL key is `<METHOD>/<path>`.
        let template_path = self.step.template.trim_matches('/');
        let dsl_key = format!("{}/{}", method, template_path);

        let target_dsl = dsls
            .get(project)
            .and_then(|by_method| by_method.get(&method))
            .and_then(|by_key| by_key.get(&dsl_key))
            .ok_or_else(|| {
                RuuterError::FileNotFound(format!(
                    "template not found: {} (project={})",
                    dsl_key, project
                ))
            })?
            .clone();

        // Build the child context with caller-provided overrides. Each
        // arg accepts either a YAML mapping (per-key `${…}` evaluated)
        // or a single `${expr}` string that resolves to an object at
        // runtime. Matches the "as if HTTP call" semantics documented
        // for the Java Ruuter template step.
        let child_body: HashMap<String, Value> = evaluate_map_arg(
            self.step.body.as_ref(),
            &self.script_engine,
            context,
            "template",
            "body",
        )?
        .unwrap_or_default();

        let child_query: HashMap<String, Value> = evaluate_map_arg(
            self.step.query.as_ref(),
            &self.script_engine,
            context,
            "template",
            "query",
        )?
        .unwrap_or_default();

        // Audit finding 06: header values are `Value` and each one
        // is passed through the script engine (matches Java's
        // `evaluateScripts(headers, …)`). Evaluated result is
        // coerced to a header-safe String — objects/arrays become
        // JSON text (same as Java's `convertMapObjectValuesToString`).
        let child_headers: HashMap<String, String> = evaluate_map_arg(
            self.step.headers.as_ref(),
            &self.script_engine,
            context,
            "template",
            "headers",
        )?
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let as_str = match v {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, as_str)
                })
                .collect()
        })
        .unwrap_or_default();

        // Fresh state store for the child? No — share the parent's
        // project-scoped state so a template's `state` step sees the
        // same view its caller does. Same for traceparent propagation.
        let mut child_ctx = ExecutionContext::with_state(
            child_body,
            child_query,
            child_headers,
            context.request_origin().to_string(),
            project.to_string(),
            share_state_store(context),
        );
        if let Some(tp) = context.traceparent() {
            child_ctx = child_ctx.with_traceparent(tp.to_string());
        }

        let result = self.engine.run(&target_dsl, &child_ctx).await?;

        // Audit finding 06: Java binds `templateInstance.getReturnValue()`
        // directly under `resultName` — the caller's `${templateVar}`
        // reads the raw return value, NOT a `.response.body` chain.
        // We match that here so ported Java DSLs work as-is.
        if let Some(result_name) = &self.step.result {
            let bound = result.value.clone().unwrap_or(Value::Null);
            context.set_variable(result_name.clone(), bound);
        }

        // Finding 03 fix: return `next` as-is (None → engine falls
        // through to source-order next). Never force `"end"`.
        let mut extras = StepLogExtras::new()
            .push("dsl", dsl_key)
            .push("status", result.status);
        // Surface a redacted preview of what the callee DSL returned
        // so the parent's trail explains WHAT it just embedded.
        // Uses the engine's logging config so redact_body_fields is
        // honoured; falls back to defaults when unset.
        if let Some(preview) = preview_body_for_log(result.value.as_ref(), &self.engine.logging()) {
            extras = extras.push_preformatted("body", preview);
        }
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

/// Clone the parent's StateStore handle. `StateStore` is internally
/// Arc-backed so this is a cheap clone that shares the underlying map.
fn share_state_store(ctx: &ExecutionContext) -> StateStore {
    ctx.state().clone()
}

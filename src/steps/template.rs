use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::state::StateStore;
use crate::steps::engine::StepEngine;
use crate::steps::{StepExecutor, StepResult, TemplateStep};
use crate::{Result, RuuterError};
use serde_json::{json, Value};
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

        // Build the child context with caller-provided overrides. Body
        // values get JS-evaluated so `${…}` inside the caller's args
        // resolves against the caller's context. Query and headers
        // start empty for the callee — this matches the "as if HTTP
        // call" semantics documented for the Java Ruuter template step.
        let child_body: HashMap<String, Value> = if let Some(body) = &self.step.body {
            let mut evaluated = HashMap::with_capacity(body.len());
            for (k, v) in body {
                evaluated.insert(k.clone(), self.script_engine.evaluate(v, context)?);
            }
            evaluated
        } else {
            HashMap::new()
        };

        let child_query: HashMap<String, Value> = if let Some(q) = &self.step.query {
            let mut evaluated = HashMap::with_capacity(q.len());
            for (k, v) in q {
                evaluated.insert(k.clone(), self.script_engine.evaluate(v, context)?);
            }
            evaluated
        } else {
            HashMap::new()
        };

        let child_headers: HashMap<String, String> = if let Some(h) = &self.step.headers {
            h.clone()
        } else {
            HashMap::new()
        };

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

        // Bind the template's return value into the caller's context
        // under the caller-provided `result` name, mirroring how the
        // http step binds its response.
        if let Some(result_name) = &self.step.result {
            let bound = json!({
                "response": {
                    "status": result.status,
                    "body": result.value,
                    "headers": result.headers,
                }
            });
            context.set_variable(result_name.clone(), bound);
        }

        Ok(StepResult::with_next(
            self.step.next.clone().unwrap_or_else(|| "end".to_string()),
        ))
    }
}

/// Clone the parent's StateStore handle. `StateStore` is internally
/// Arc-backed so this is a cheap clone that shares the underlying map.
fn share_state_store(ctx: &ExecutionContext) -> StateStore {
    ctx.state().clone()
}

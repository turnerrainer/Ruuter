use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::engine::StepEngine;
use crate::steps::{IterateStep, StepExecutor, StepLogExtras, StepResult};
use crate::{Result, RuuterError};
use serde_json::Value;

const DEFAULT_MAX_ITEMS: usize = 10_000;

pub struct IterateStepExecutor {
    step: IterateStep,
    engine: StepEngine,
    script_engine: ScriptEngine,
}

impl IterateStepExecutor {
    pub fn new(step: IterateStep, engine: StepEngine) -> Self {
        Self {
            step,
            engine,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for IterateStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let body = &self.step.iterate;

        // Evaluate `over` to a list. The expression sees whatever's
        // already in the context.
        let list_value = self.script_engine.evaluate(&body.over, context)?;
        let items = match list_value {
            Value::Array(arr) => arr,
            other => {
                return Err(RuuterError::InvalidStep(format!(
                    "iterate.over must evaluate to an array, got {:?}",
                    other
                )));
            }
        };

        let max = body.max_items.unwrap_or(DEFAULT_MAX_ITEMS);
        if items.len() > max {
            return Err(RuuterError::InvalidStep(format!(
                "iterate.over has {} items but max_items is {}",
                items.len(),
                max
            )));
        }

        let item_count = items.len();
        let mut collected: Vec<Value> = Vec::with_capacity(item_count);

        for item in items {
            // Bind the item under `body.item_var`.
            context.set_variable(body.item_var.clone(), item.clone());

            // Run each body step once, sequentially. We intentionally
            // ignore `next:` directives inside iterate body — they
            // would conflict with iteration semantics. Use a Return
            // step to bail early.
            for sub_step in &body.body {
                let result = self.engine.execute_single_step(sub_step, context).await?;
                if result.should_return {
                    // Early return propagates up — the outer DSL stops
                    // iterating and returns this result.
                    return Ok(result);
                }
            }

            // Optional aggregation.
            if let Some(collect_expr) = &body.collect {
                let v = self.script_engine.evaluate(collect_expr, context)?;
                collected.push(v);
            }
        }

        if let Some(into) = &body.into {
            context.set_variable(into.clone(), Value::Array(collected));
        }

        let extras = StepLogExtras::new()
            .push("count", item_count as u64)
            .push("as", body.item_var.clone());
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

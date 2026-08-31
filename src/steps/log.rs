use crate::context::ExecutionContext;
use crate::logging::sanitize_log_value;
use crate::scripting::ScriptEngine;
use crate::steps::{LogStep, StepExecutor, StepLogExtras, StepResult};
use crate::Result;
use tracing::info;

pub struct LogStepExecutor {
    step: LogStep,
    script_engine: ScriptEngine,
}

impl LogStepExecutor {
    pub fn new(step: LogStep) -> Self {
        Self {
            step,
            script_engine: ScriptEngine::new(),
        }
    }
}

impl StepExecutor for LogStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let message = self
            .script_engine
            .evaluate(&serde_json::Value::String(self.step.log.clone()), context)?;

        // Emit as a structured event so JSON-formatter deployments
        // get a proper field (`dsl.log`) instead of an interpolated
        // "LOG: …" string blob. Text-formatter renders it as
        // `dsl.log=<message>` on the same line — visually similar to
        // Java's `LOG: <message>` but attacker-controlled newlines
        // are stripped to prevent log-line splicing.
        let rendered = message.as_str().map(sanitize_log_value).unwrap_or_else(|| {
            sanitize_log_value(&serde_json::to_string(&message).unwrap_or_default())
        });
        info!(
            dsl.project = %context.project(),
            dsl.log = %rendered,
            "dsl log step"
        );

        // Also surface the evaluated message on the engine's
        // per-step "Executed" line (issue #37) so a reader scanning
        // the DSL trail sees WHAT was logged without correlating two
        // lines. Capped to keep the trail line bounded.
        let for_extras = if rendered.len() > 256 {
            let mut cut = 256;
            while cut > 0 && !rendered.is_char_boundary(cut) {
                cut -= 1;
            }
            let mut s = rendered[..cut].to_string();
            s.push('…');
            s
        } else {
            rendered
        };
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: StepLogExtras::new().push("msg", for_extras),
            ..StepResult::new()
        })
    }
}

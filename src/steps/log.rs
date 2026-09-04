use crate::context::ExecutionContext;
use crate::logging::sanitize_log_value;
use crate::scripting::ScriptEngine;
use crate::steps::{LogStep, StepExecutor, StepLogExtras, StepResult};
use crate::Result;

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
        // Issue #56 — `log:` accepts any Value (scalar/map/array).
        // ScriptEngine::evaluate walks objects and arrays and
        // interpolates `${…}` on string leaves, so a mapping-form
        // log payload evaluates the same as an assign body.
        let message = self.script_engine.evaluate(&self.step.log, context)?;

        // Attacker-controlled CR/LF stripped to prevent log-line
        // splicing before the message reaches any log sink.
        let rendered = message.as_str().map(sanitize_log_value).unwrap_or_else(|| {
            sanitize_log_value(&serde_json::to_string(&message).unwrap_or_default())
        });

        // Surface the evaluated message on the engine's per-step
        // "Executed" line (issue #37) via `attrs.msg` — that line is
        // the single carrier for the log-step payload in both text
        // and JSON output. A separate `dsl log step` event used to
        // fire here too; removed because it doubled every log step
        // in the trail without adding a new signal.
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

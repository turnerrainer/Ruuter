use crate::context::ExecutionContext;
use crate::{Result, RuuterError};
use boa_engine::{Context as BoaContext, JsValue, Source};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

pub struct ScriptEngine {
    script_pattern: Regex,
    line_pattern: Regex,
}

impl ScriptEngine {
    pub fn new() -> Self {
        Self {
            script_pattern: Regex::new(r"\$\{([^}]+)\}").unwrap(),
            line_pattern: Regex::new(r"\$=(.+)=$").unwrap(),
        }
    }

    pub fn evaluate(&self, input: &Value, context: &ExecutionContext) -> Result<Value> {
        match input {
            Value::String(s) => self.evaluate_string(s, context),
            Value::Object(map) => {
                let mut result = serde_json::Map::new();
                for (k, v) in map {
                    result.insert(k.clone(), self.evaluate(v, context)?);
                }
                Ok(Value::Object(result))
            }
            Value::Array(arr) => {
                let mut result = Vec::new();
                for v in arr {
                    result.push(self.evaluate(v, context)?);
                }
                Ok(Value::Array(result))
            }
            _ => Ok(input.clone()),
        }
    }

    fn evaluate_string(&self, s: &str, context: &ExecutionContext) -> Result<Value> {
        // Check if entire string is a single expression
        if let Some(caps) = self.script_pattern.captures(s) {
            if caps.get(0).unwrap().as_str() == s {
                return self.execute_js(&caps[1], context);
            }
        }

        if let Some(caps) = self.line_pattern.captures(s) {
            if caps.get(0).unwrap().as_str() == s {
                return self.execute_js(&caps[1], context);
            }
        }

        // Replace all expressions in string
        let result = self.script_pattern.replace_all(s, |caps: &regex::Captures| {
            match self.execute_js(&caps[1], context) {
                Ok(Value::String(s)) => s,
                Ok(v) => v.to_string(),
                Err(_) => caps[0].to_string(),
            }
        });

        Ok(Value::String(result.to_string()))
    }

    fn execute_js(&self, script: &str, context: &ExecutionContext) -> Result<Value> {
        let mut boa_context = BoaContext::default();

        // Set up context bindings
        self.setup_bindings(&mut boa_context, context)?;

        // Execute script
        let source = Source::from_bytes(script);
        let result = boa_context.eval(source)
            .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;

        // Convert result to JSON Value
        self.js_value_to_json(&result, &mut boa_context)
    }

    fn setup_bindings(&self, boa_context: &mut BoaContext, context: &ExecutionContext) -> Result<()> {
        // Create incoming object
        let mut incoming = HashMap::new();
        incoming.insert("params", Value::Object(
            context.request_query().iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        ));
        incoming.insert("body", Value::Object(
            context.request_body().iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        ));
        incoming.insert("headers", Value::Object(
            context.request_headers().iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect()
        ));

        let incoming_json = serde_json::to_string(&incoming)?;
        let script = format!("var incoming = {};", incoming_json);
        boa_context.eval(Source::from_bytes(&script))
            .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;

        // Add context variables
        for (key, value) in context.get_all_variables() {
            let value_json = serde_json::to_string(&value)?;
            let script = format!("var {} = {};", key, value_json);
            boa_context.eval(Source::from_bytes(&script))
                .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
        }

        Ok(())
    }

    fn js_value_to_json(&self, value: &JsValue, boa_context: &mut BoaContext) -> Result<Value> {
        if value.is_null() || value.is_undefined() {
            return Ok(Value::Null);
        }

        if let Some(b) = value.as_boolean() {
            return Ok(Value::Bool(b));
        }

        if let Some(n) = value.as_number() {
            if n.fract() == 0.0 && n.is_finite() {
                return Ok(Value::Number(serde_json::Number::from(n as i64)));
            }
            return Ok(Value::Number(
                serde_json::Number::from_f64(n)
                    .ok_or_else(|| RuuterError::ScriptEvaluation("Invalid number".to_string()))?
            ));
        }

        if let Some(s) = value.as_string() {
            return Ok(Value::String(s.to_std_string_escaped()));
        }

        // Try to stringify as JSON
        let json_str = value.to_json(boa_context)
            .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?
            .to_string();

        serde_json::from_str(&json_str)
            .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

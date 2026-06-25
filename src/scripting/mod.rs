use crate::context::ExecutionContext;
use crate::{Result, RuuterError};
use boa_engine::{Context as BoaContext, JsValue, Source};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

static SCRIPT_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{([^}]+)\}").unwrap());
static LINE_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$=(.+)=$").unwrap());

pub struct ScriptEngine;

impl ScriptEngine {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate `input` against `context`. Builds a single Boa context for the
    /// duration of this call and reuses it across every `${...}` / `$=...=$`
    /// expression found inside `input` (recursing through objects and arrays).
    pub fn evaluate(&self, input: &Value, context: &ExecutionContext) -> Result<Value> {
        let mut boa = BoaContext::default();
        setup_bindings(&mut boa, context)?;
        evaluate_with(input, &mut boa)
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn evaluate_with(input: &Value, boa: &mut BoaContext) -> Result<Value> {
    match input {
        Value::String(s) => evaluate_string(s, boa),
        Value::Object(map) => {
            let mut result = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                result.insert(k.clone(), evaluate_with(v, boa)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(arr) => {
            let mut result = Vec::with_capacity(arr.len());
            for v in arr {
                result.push(evaluate_with(v, boa)?);
            }
            Ok(Value::Array(result))
        }
        _ => Ok(input.clone()),
    }
}

fn evaluate_string(s: &str, boa: &mut BoaContext) -> Result<Value> {
    // Whole-string single-expression form: `${...}` — preserve native type.
    if let Some(caps) = SCRIPT_PATTERN.captures(s) {
        if caps.get(0).unwrap().as_str() == s {
            return execute_js(&caps[1], boa);
        }
    }
    if let Some(caps) = LINE_PATTERN.captures(s) {
        if caps.get(0).unwrap().as_str() == s {
            return execute_js(&caps[1], boa);
        }
    }

    // Mixed string — interpolate every `${...}` and stringify the result.
    let mut last_err: Option<RuuterError> = None;
    let replaced = SCRIPT_PATTERN.replace_all(s, |caps: &regex::Captures| {
        match execute_js(&caps[1], boa) {
            Ok(Value::String(out)) => out,
            Ok(other) => other.to_string(),
            Err(e) => {
                if last_err.is_none() {
                    last_err = Some(e);
                }
                caps[0].to_string()
            }
        }
    });

    if let Some(e) = last_err {
        return Err(e);
    }
    Ok(Value::String(replaced.to_string()))
}

fn execute_js(script: &str, boa: &mut BoaContext) -> Result<Value> {
    let source = Source::from_bytes(script);
    let result = boa.eval(source)
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
    js_value_to_json(&result, boa)
}

fn setup_bindings(boa: &mut BoaContext, context: &ExecutionContext) -> Result<()> {
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
    incoming.insert("connection_id", match context.connection_id() {
        Some(id) => Value::String(id.to_string()),
        None => Value::Null,
    });

    let incoming_json = serde_json::to_string(&incoming)?;
    boa.eval(Source::from_bytes(&format!("var incoming = {};", incoming_json)))
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;

    for (key, value) in context.get_all_variables() {
        let value_json = serde_json::to_string(&value)?;
        boa.eval(Source::from_bytes(&format!("var {} = {};", key, value_json)))
            .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
    }

    Ok(())
}

fn js_value_to_json(value: &JsValue, boa: &mut BoaContext) -> Result<Value> {
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

    // Arrays first — Boa's `to_json` returns Value::Object({}) for top-
    // level JS arrays. Iterate via length + indexed access to preserve
    // the array shape (including the empty case).
    if let Some(obj) = value.as_object() {
        if obj.is_array() {
            let length_val = obj
                .get(boa_engine::js_string!("length"), boa)
                .map_err(|e| RuuterError::ScriptEvaluation(format!("array length: {}", e)))?;
            let length = length_val.as_number().unwrap_or(0.0) as usize;
            let mut arr = Vec::with_capacity(length);
            for i in 0..length {
                let key = boa_engine::js_string!(i.to_string());
                let item = obj
                    .get(key, boa)
                    .map_err(|e| RuuterError::ScriptEvaluation(format!("array[{}]: {}", i, e)))?;
                arr.push(js_value_to_json(&item, boa)?);
            }
            return Ok(Value::Array(arr));
        }
    }

    let json_str = value.to_json(boa)
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?
        .to_string();
    serde_json::from_str(&json_str)
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))
}

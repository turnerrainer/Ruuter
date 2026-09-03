//! Boa backend for `ScriptEngine` (task 051 refactor).
//!
//! Everything Boa-specific lives here. The engine-agnostic shell in
//! `super` re-exports this type as `super::ScriptEngine` when the
//! `scripting-boa` feature is enabled (default).

use super::{
    bump_context_created, find_script_segments, has_expressions, ScriptLimits, DEFAULT_LIMITS,
    LINE_PATTERN,
};
use crate::context::ExecutionContext;
use crate::{Result, RuuterError};
use boa_engine::{property::Attribute, Context as BoaContext, JsValue, Source};
use serde_json::Value;
use std::collections::HashMap;

pub struct BoaScriptEngine {
    limits: ScriptLimits,
}

impl BoaScriptEngine {
    pub fn new() -> Self {
        let limits = DEFAULT_LIMITS.get().copied().unwrap_or_default();
        Self { limits }
    }

    pub fn with_limits(limits: ScriptLimits) -> Self {
        Self { limits }
    }

    /// Evaluate `input` against `context`. Builds a single Boa
    /// context for the duration of this call and reuses it across
    /// every `${...}` / `$= expr =` expression found inside `input`
    /// (recursing through objects and arrays).
    ///
    /// Task 037 fast-path: if `input` (recursively) contains no
    /// `${...}` or whole-string `$= expr =` expressions, return
    /// `input.clone()` without constructing a Boa context.
    pub fn evaluate(&self, input: &Value, context: &ExecutionContext) -> Result<Value> {
        self.evaluate_tracked(input, context).map(|(v, _)| v)
    }

    /// Same as [`evaluate`], but also returns whether the engine
    /// was actually invoked. Used by task 037 tests.
    pub fn evaluate_tracked(
        &self,
        input: &Value,
        context: &ExecutionContext,
    ) -> Result<(Value, bool)> {
        if !has_expressions(input) {
            return Ok((input.clone(), false));
        }

        bump_context_created();

        let mut boa = BoaContext::default();
        boa.runtime_limits_mut()
            .set_loop_iteration_limit(self.limits.max_loop_iterations);
        boa.runtime_limits_mut()
            .set_recursion_limit(self.limits.max_stack_size);
        setup_bindings(&mut boa, context)?;
        let out = evaluate_with(input, &mut boa)?;
        Ok((out, true))
    }
}

impl Default for BoaScriptEngine {
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
    // Whole-string single-expression form: `${...}` — preserve
    // native type.
    let segs = find_script_segments(s);
    if segs.len() == 1 {
        let (start, end, ref inner) = segs[0];
        if start == 0 && end == s.len() {
            return maybe_suppress_optional_null(execute_js(inner, boa)?, inner);
        }
    }
    if let Some(caps) = LINE_PATTERN.captures(s) {
        if caps.get(0).unwrap().as_str() == s {
            let inner = &caps[1];
            return maybe_suppress_optional_null(execute_js(inner, boa)?, inner);
        }
    }

    // Mixed string — interpolate every `${...}` and stringify.
    let mut last_err: Option<RuuterError> = None;
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    for (start, end, inner) in &segs {
        out.push_str(&s[cursor..*start]);
        match execute_js(inner, boa) {
            Ok(v) => {
                // Audit finding 17: `.optional.` in the expression
                // text suppresses null → "" (Java's
                // filterEmptyOptional). Applied per-segment for
                // mixed strings.
                let coerced = maybe_suppress_optional_null(v, inner)?;
                match coerced {
                    Value::String(s2) => out.push_str(&s2),
                    // Issue #57 — `null` / `undefined` interpolate as
                    // empty, not the literal strings "null" / "undefined".
                    // A DSL that writes `"Hello ${name}"` with `name`
                    // undeclared should yield `"Hello "`, not
                    // `"Hello null"`. Matches the same value's fate in
                    // headers (dropped) and bodies (omitted from
                    // objects, preserved in arrays as null).
                    Value::Null => {}
                    other => out.push_str(&other.to_string()),
                }
            }
            Err(e) => {
                if last_err.is_none() {
                    last_err = Some(e);
                }
                out.push_str(&s[*start..*end]);
            }
        }
        cursor = *end;
    }
    out.push_str(&s[cursor..]);

    if let Some(e) = last_err {
        return Err(e);
    }
    Ok(Value::String(out))
}

/// Audit finding 17 — Java's `filterEmptyOptional`. When a script
/// expression's text contains `.optional.` or `.optional_`, a null
/// / undefined result is coerced to `""` (empty string). This lets
/// DSL authors reach optional response fields without wrapping each
/// access in a `switch:` guard:
///
/// ```yaml
/// assign:
///   tag: "${incoming.body.optional.tag}"   # missing → "", not null
/// ```
///
/// Rust exposes evaluated null as `Value::Null` — coerce to empty
/// string when the expression text opts in.
fn maybe_suppress_optional_null(v: Value, expr: &str) -> Result<Value> {
    if matches!(v, Value::Null) && (expr.contains(".optional.") || expr.contains(".optional_")) {
        return Ok(Value::String(String::new()));
    }
    Ok(v)
}

fn execute_js(script: &str, boa: &mut BoaContext) -> Result<Value> {
    // Issue #57 — a `${platform?.id}` where `platform` was never
    // declared should evaluate to `undefined` (→ `Value::Null` at the
    // JSON boundary), not throw ReferenceError. This makes template
    // composition tractable: a caller can reference optional bindings
    // without every DSL author having to `assign` a placeholder first.
    //
    // The wrap swallows only `ReferenceError`, so TypeError from
    // `foo.bar` where `foo` IS declared but is `null`/`undefined`
    // still surfaces — that's a real bug in the DSL, not a missing
    // binding. `.call(globalThis)` keeps `this === globalThis` inside
    // the script body, matching QuickJS and Ruuter's audit finding 16
    // contract that `${this['foo-bar']}` reads work.
    let wrapped = format!(
        "(function(){{ try {{ return ({}); }} catch(e) {{ if (e instanceof ReferenceError) return undefined; throw e; }} }}).call(globalThis)",
        script
    );
    let source = Source::from_bytes(wrapped.as_bytes());
    let result = boa
        .eval(source)
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
    js_value_to_json(&result, boa)
}

fn setup_bindings(boa: &mut BoaContext, context: &ExecutionContext) -> Result<()> {
    let mut incoming = HashMap::new();
    incoming.insert(
        "params",
        Value::Object(
            context
                .request_query()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
    );
    incoming.insert(
        "body",
        Value::Object(
            context
                .request_body()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
    );
    incoming.insert(
        "headers",
        Value::Object(
            context
                .request_headers()
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        ),
    );
    incoming.insert(
        "connection_id",
        match context.connection_id() {
            Some(id) => Value::String(id.to_string()),
            None => Value::Null,
        },
    );
    // h2ck.me S4 — expose the framework-computed origin (peer IP, or
    // an X-Forwarded-For value from a trusted proxy). Distinct from
    // `incoming.headers["x-forwarded-for"]`, which is the raw
    // client-controlled value.
    incoming.insert(
        "origin",
        Value::String(context.request_origin().to_string()),
    );

    let incoming_json = serde_json::to_string(&incoming)?;
    boa.eval(Source::from_bytes(&format!(
        "var incoming = {};",
        incoming_json
    )))
    .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;

    // Audit finding 16: bind variables via `globalThis["<name>"] = …`
    // rather than `var <name> = …`. Pre-fix, DSL variable names
    // that aren't valid JS identifiers (dashes, dots, spaces) made
    // the eval throw SyntaxError before the actual script even
    // ran. Under this fix the binding always succeeds; identifier-
    // valid names still resolve via bare `${foo}` (globalThis
    // lookup) while non-identifier names remain reachable via
    // `${this["foo-bar"]}` in the DSL.
    for (key, value) in context.get_all_variables() {
        let value_json = serde_json::to_string(&value)?;
        boa.eval(Source::from_bytes(&format!(
            "globalThis[{}] = {};",
            js_string_literal(&key),
            value_json
        )))
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
    }

    Ok(())
}

/// Audit finding 16 helper. Encode `s` as a JS string literal with
/// proper escaping so untrusted variable names can't inject syntax
/// into the eval string.
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
        return Ok(Value::Number(serde_json::Number::from_f64(n).ok_or_else(
            || RuuterError::ScriptEvaluation("Invalid number".to_string()),
        )?));
    }

    if let Some(s) = value.as_string() {
        return Ok(Value::String(s.to_std_string_escaped()));
    }

    // Arrays first — Boa's `to_json` returns Value::Object({}) for
    // top-level JS arrays. Iterate via length + indexed access to
    // preserve the array shape.
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

    // boa's built-in `to_json` panics on any object with a nested
    // `undefined` value. Round-trip through `JSON.stringify` — spec
    // says undefined properties drop from objects, undefined array
    // slots become null. Slot is non-writable so a script can't
    // hijack it mid-evaluation, and CONFIGURABLE so back-to-back
    // fallbacks can rebind.
    boa.register_global_property(
        boa_engine::js_string!("__ruuter_serialize_tmp__"),
        value.clone(),
        Attribute::CONFIGURABLE,
    )
    .map_err(|e| RuuterError::ScriptEvaluation(format!("serialize slot: {}", e)))?;
    let stringified = boa
        .eval(Source::from_bytes(
            "JSON.stringify(__ruuter_serialize_tmp__)",
        ))
        .map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))?;
    if stringified.is_undefined() {
        return Ok(Value::Null);
    }
    let json_str = stringified
        .as_string()
        .ok_or_else(|| {
            RuuterError::ScriptEvaluation("JSON.stringify returned non-string".to_string())
        })?
        .to_std_string_escaped();
    serde_json::from_str(&json_str).map_err(|e| RuuterError::ScriptEvaluation(e.to_string()))
}

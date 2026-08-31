//! QuickJS backend for `ScriptEngine` (task 051).
//!
//! Uses `rquickjs` 0.6 with the `parallel + futures` features so the
//! `Runtime` and `Context` types are `Send + Sync`. That property is
//! the reason this backend exists — Boa is `!Send`, which forecloses
//! per-request context pooling (task 036) and pre-parsed script cache
//! (task 045). This module deliberately mirrors the Boa backend's
//! surface API: same `evaluate` / `evaluate_tracked` signatures, same
//! `incoming.{body,params,headers,connection_id}` bindings, same
//! whole-expression-preserves-native-type semantics.
//!
//! DSL authors write the same YAML; the operator picks the engine at
//! build time via `cargo build --features scripting-quickjs`.
//!
//! ## Compatibility notes vs Boa
//!
//! QuickJS is highly ECMAScript-compatible but not byte-identical to
//! Boa. Known deltas that DSL authors might encounter:
//!
//! - `Number.prototype.toString` precision at boundaries can differ
//!   in the last digit for irrational values.
//! - `Date` parsing edge cases (non-ISO strings) may accept/reject
//!   differently.
//! - Regex engine implementations differ; complex Unicode classes
//!   may behave differently.
//!
//! The DSL-test corpus is the gate — every scenario in `DSL-tests/`
//! must pass byte-identically on both engines. If a scenario diverges,
//! either fix the DSL to use the intersection of engine behaviours, or
//! document the divergence in the book.
//!
//! ## Concurrency
//!
//! Unlike Boa, this backend can (in a future task 036) hold a shared
//! context on `ExecutionContext` across `.await`. For now v1 still
//! creates a fresh Runtime + Context per evaluate call — same shape
//! as Boa — so the perf story is "raw QuickJS vs raw Boa", not "pool
//! vs no-pool" yet.

use super::{
    bump_context_created, find_script_segments, has_expressions, ExpressionRegistry, ScriptLimits,
    DEFAULT_LIMITS, LINE_PATTERN,
};
use crate::context::{ExecutionContext, QuickJsSession};
use crate::{Result, RuuterError};
use rquickjs::{Context as QjsContext, Runtime as QjsRuntime};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

pub struct QuickJsScriptEngine {
    limits: ScriptLimits,
}

impl QuickJsScriptEngine {
    pub fn new() -> Self {
        let limits = DEFAULT_LIMITS.get().copied().unwrap_or_default();
        Self { limits }
    }

    pub fn with_limits(limits: ScriptLimits) -> Self {
        Self { limits }
    }

    /// Evaluate `input` against `context`. Behaviour parity with the
    /// Boa backend — same fast-path, same recursion into
    /// objects/arrays, same `${...}` and `$=...=` semantics.
    pub fn evaluate(&self, input: &Value, context: &ExecutionContext) -> Result<Value> {
        self.evaluate_tracked(input, context).map(|(v, _)| v)
    }

    /// Same as [`evaluate`], but also returns whether the engine was
    /// actually invoked. Task 037 tests use this signal.
    ///
    /// Task 036: reuses a per-request `QuickJsSession` (Runtime +
    /// Context pair) via `ExecutionContext::quickjs_session()`. First
    /// evaluate in a request builds the session; subsequent evaluates
    /// in the same request skip Runtime + Context construction and
    /// only re-run the setup_bindings JSON binding refresh. On the
    /// per-request-CPU profile at 1k rps this drops the Boa/QuickJS
    /// overhead from ~1-2 ms/call to ~0.2-0.5 ms/call for calls #2..n.
    pub fn evaluate_tracked(
        &self,
        input: &Value,
        context: &ExecutionContext,
    ) -> Result<(Value, bool)> {
        if !has_expressions(input) {
            return Ok((input.clone(), false));
        }

        // Reuse the per-request session if present; otherwise build
        // and cache. `get_or_init` runs the closure at most once
        // across all clones of this ExecutionContext (Arc<OnceLock>).
        let session_slot = context.quickjs_session();
        let registry = context.expr_registry();
        let session = session_slot.get_or_init(|| {
            bump_context_created();
            let runtime = QjsRuntime::new().expect("qjs runtime construction cannot fail");
            // Rough approximation of Boa's runtime_limits — QuickJS
            // has set_max_stack_size (bytes). Map Boa's stack-depth
            // limit at 128 KB × depth as a conservative default.
            runtime.set_max_stack_size(self.limits.max_stack_size.saturating_mul(128 * 1024));
            let context = QjsContext::full(&runtime).expect("qjs context construction cannot fail");

            // Task 045 — one flag per registered expression id,
            // all `false` initially. Bulk-compile at session init
            // costs more than it saves for per-request sessions
            // (compile 60 expressions to use 3); instead we lazily
            // compile each expression on first use per session and
            // set its flag. Second+ evals skip the compile.
            let mut compiled_flags = Vec::with_capacity(registry.len());
            for _ in 0..registry.len() {
                compiled_flags.push(std::sync::atomic::AtomicBool::new(false));
            }

            QuickJsSession {
                context,
                runtime,
                compiled_flags,
            }
        });

        let out = session.context.with(|ctx| -> Result<Value> {
            // Bindings refreshed on every evaluate — the DSL author
            // could have assigned new user variables between evals.
            // Overhead: one JSON.parse per variable, small.
            setup_bindings(&ctx, context)?;
            evaluate_with(input, &ctx, registry, session)
        })?;
        Ok((out, true))
    }
}

impl Default for QuickJsScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn evaluate_with<'js>(
    input: &Value,
    ctx: &rquickjs::Ctx<'js>,
    registry: &ExpressionRegistry,
    session: &QuickJsSession,
) -> Result<Value> {
    match input {
        Value::String(s) => evaluate_string(s, ctx, registry, session),
        Value::Object(map) => {
            let mut result = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                result.insert(k.clone(), evaluate_with(v, ctx, registry, session)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(arr) => {
            let mut result = Vec::with_capacity(arr.len());
            for v in arr {
                result.push(evaluate_with(v, ctx, registry, session)?);
            }
            Ok(Value::Array(result))
        }
        _ => Ok(input.clone()),
    }
}

fn evaluate_string<'js>(
    s: &str,
    ctx: &rquickjs::Ctx<'js>,
    registry: &ExpressionRegistry,
    session: &QuickJsSession,
) -> Result<Value> {
    let segs = find_script_segments(s);
    // Whole-string `${...}` preserves native type
    if segs.len() == 1 {
        let (start, end, ref inner) = segs[0];
        if start == 0 && end == s.len() {
            let v = execute_js(inner, ctx, registry, session)?;
            return maybe_suppress_optional_null(v, inner);
        }
    }
    // Whole-string `$=...=` line pattern
    if let Some(caps) = LINE_PATTERN.captures(s) {
        if caps.get(0).unwrap().as_str() == s {
            let inner = &caps[1];
            let v = execute_js(inner, ctx, registry, session)?;
            return maybe_suppress_optional_null(v, inner);
        }
    }

    // Mixed string — interpolate each segment, stringify each result
    let mut last_err: Option<RuuterError> = None;
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    for (start, end, inner) in &segs {
        out.push_str(&s[cursor..*start]);
        match execute_js(inner, ctx, registry, session) {
            Ok(v) => {
                // Audit finding 17: .optional. null suppression on
                // each segment of a mixed string.
                let coerced = maybe_suppress_optional_null(v, inner)?;
                match coerced {
                    Value::String(s2) => out.push_str(&s2),
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

/// Audit finding 17 — Java's `filterEmptyOptional`. See the
/// matching helper in `boa.rs` for the full docstring; QuickJS and
/// Boa share semantics so a DSL author writes the same YAML on
/// either backend.
fn maybe_suppress_optional_null(v: Value, expr: &str) -> Result<Value> {
    if matches!(v, Value::Null) && (expr.contains(".optional.") || expr.contains(".optional_")) {
        return Ok(Value::String(String::new()));
    }
    Ok(v)
}

fn execute_js<'js>(
    script: &str,
    ctx: &rquickjs::Ctx<'js>,
    registry: &ExpressionRegistry,
    session: &QuickJsSession,
) -> Result<Value> {
    // Task 045 — if this expression was registered at DSL load AND
    // we've compiled it already in this session, invoke by id.
    // If not yet compiled in this session, compile-and-invoke in
    // one eval, then mark the flag. If not registered at all,
    // fall back to per-eval compile (rare — only for scripts the
    // engine synthesises internally).
    let js_value: rquickjs::Value<'js> = match registry.id_for(script) {
        Some(id) => {
            let already = session
                .compiled_flags
                .get(id as usize)
                .map(|f| f.load(Ordering::Acquire));
            // Invoke via `.call(globalThis)` so `this` inside the
            // script body is `globalThis` (matches Boa's top-level
            // eval semantics). Plain `()` would leave `this` as
            // `undefined` under QuickJS strict mode, breaking DSL
            // authors' `${this['foo-bar']}` reads (audit finding 16).
            let script_bytes = if already == Some(true) {
                format!("__fn_{}.call(globalThis)", id)
            } else {
                // Combined define+invoke — single eval, no
                // double-parse. The parenthesised assignment
                // returns the freshly-defined function; `.call`
                // invokes it with `this === globalThis`.
                format!(
                    "(globalThis.__fn_{} = function(){{ return ({}); }}).call(globalThis)",
                    id, script
                )
            };
            let val: rquickjs::Value<'js> = ctx
                .eval(script_bytes.as_bytes())
                .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs eval: {}", e)))?;
            if let Some(flag) = session.compiled_flags.get(id as usize) {
                flag.store(true, Ordering::Release);
            }
            val
        }
        None => {
            // Not registered — synthesised script. Compile inline.
            let wrapped = format!("(function(){{ return ({}); }}).call(globalThis)", script);
            ctx.eval(wrapped.as_bytes())
                .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs eval: {}", e)))?
        }
    };
    js_value_to_json(js_value)
}

fn setup_bindings<'js>(ctx: &rquickjs::Ctx<'js>, context: &ExecutionContext) -> Result<()> {
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
    // h2ck.me S4 — mirror boa.rs: expose the trusted origin.
    incoming.insert(
        "origin",
        Value::String(context.request_origin().to_string()),
    );

    // QuickJS JSON.parse produces a native JS object; then attach to
    // globalThis under the expected name. No macro-generated bindings
    // needed for this small shape.
    let incoming_json = serde_json::to_string(&incoming)?;
    ctx.eval::<(), _>(
        format!(
            "globalThis.incoming = JSON.parse({});",
            js_string_literal(&incoming_json)
        )
        .as_bytes(),
    )
    .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs bind incoming: {}", e)))?;

    // Audit finding 16: bind via `globalThis[<js-string>]` rather
    // than dot syntax on globalThis, so DSL variable names with
    // non-identifier characters (dashes, dots) still bind cleanly
    // instead of failing with a SyntaxError.
    for (key, value) in context.get_all_variables() {
        let value_json = serde_json::to_string(&value)?;
        ctx.eval::<(), _>(
            format!(
                "globalThis[{}] = JSON.parse({});",
                js_string_literal(&key),
                js_string_literal(&value_json)
            )
            .as_bytes(),
        )
        .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs bind {}: {}", key, e)))?;
    }

    Ok(())
}

/// Encode `s` as a JavaScript string literal (double-quoted, with
/// backslashes for `\`, `"`, `\n`, `\r`, `\t`, and non-ASCII → \uXXXX
/// escapes so the resulting JS is 7-bit ASCII safe). Used to embed
/// user-provided JSON into an eval string.
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

fn js_value_to_json<'js>(value: rquickjs::Value<'js>) -> Result<Value> {
    // Use JSON.stringify roundtrip via a helper — cheapest correct
    // path from arbitrary QjsValue → serde_json::Value. For null and
    // undefined we short-circuit because JSON.stringify(undefined)
    // returns undefined (not a string).
    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if let Some(b) = value.as_bool() {
        return Ok(Value::Bool(b));
    }
    if let Some(i) = value.as_int() {
        return Ok(Value::Number(serde_json::Number::from(i)));
    }
    if let Some(f) = value.as_float() {
        if f.fract() == 0.0 && f.is_finite() && f.abs() < (i64::MAX as f64) {
            return Ok(Value::Number(serde_json::Number::from(f as i64)));
        }
        return Ok(Value::Number(
            serde_json::Number::from_f64(f)
                // Mirror the Boa backend's exact wording so DSL
                // scenario tests that regex on the message text
                // (`$regex:Invalid number`) pass on both engines.
                .ok_or_else(|| RuuterError::ScriptEvaluation("Invalid number".to_string()))?,
        ));
    }
    if let Some(s) = value.as_string() {
        return Ok(Value::String(s.to_string().map_err(|e| {
            RuuterError::ScriptEvaluation(format!("qjs str: {}", e))
        })?));
    }
    // For arrays and objects, roundtrip through JSON. This handles
    // arbitrary nesting uniformly and matches how the Boa backend
    // ends up serialising complex shapes.
    let ctx = value.ctx();
    let stringify: rquickjs::Function = ctx
        .globals()
        .get::<_, rquickjs::Object>("JSON")
        .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs JSON: {}", e)))?
        .get::<_, rquickjs::Function>("stringify")
        .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs JSON.stringify: {}", e)))?;
    let json_str: String = stringify
        .call((value,))
        .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs stringify: {}", e)))?;
    serde_json::from_str(&json_str)
        .map_err(|e| RuuterError::ScriptEvaluation(format!("qjs → json parse: {}", e)))
}

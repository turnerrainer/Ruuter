//! Scripting engine — evaluate `${expr}` and `$=expr=` inside DSL values.
//!
//! Task 051 split this module into an engine-agnostic shell plus two
//! feature-gated backends. Exactly one backend is compiled into any
//! given build via mutually-exclusive Cargo features:
//!
//! - `scripting-boa` (default) — Boa 0.19, pure Rust, no C deps.
//!   `!Send` internals block per-request context pooling. Best for
//!   small/simple DSLs that need zero-CVE-surface JS.
//! - `scripting-quickjs` — rquickjs (parallel+futures features),
//!   thin wrapper over the QuickJS C engine. Send-compatible, so
//!   per-request pools (task 036) and pre-parsed script cache
//!   (task 045) become straightforward. ~2-5× faster on typical
//!   workloads. Adds ~500 KB binary size.
//!
//! Public API is identical on both: `ScriptEngine::new()`,
//! `.evaluate(input, ctx)`, `.evaluate_tracked(input, ctx)`,
//! `install_default_limits()`, `boa_context_created_count()`. DSL
//! authors and framework callers don't need to know which backend
//! is running.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

// ── Compile-time feature check ────────────────────────────────────
// Exactly one backend must be enabled — otherwise there is no
// `ScriptEngine` to construct and every caller fails to compile.
// Emit a clear error rather than a spray of unresolved-symbol
// noise.

#[cfg(all(feature = "scripting-boa", feature = "scripting-quickjs"))]
compile_error!(
    "features `scripting-boa` and `scripting-quickjs` are mutually exclusive — enable one, not both"
);
#[cfg(not(any(feature = "scripting-boa", feature = "scripting-quickjs")))]
compile_error!(
    "no scripting backend enabled — pass `--features scripting-boa` or `--features scripting-quickjs`"
);

// ── Engine-agnostic helpers ───────────────────────────────────────

pub(crate) static LINE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$=(.+)=$").unwrap());

/// Task 037 metric — count of native JS contexts constructed since
/// process start. Both backends bump the same counter so downstream
/// tooling (tests, ops metrics) works uniformly.
///
/// Only bumped when `evaluate()` fell OFF the literal fast-path and
/// had to build an engine. Tests use this to verify the engine is
/// not invoked for expression-free values.
static CONTEXT_CREATED_COUNT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn bump_context_created() {
    CONTEXT_CREATED_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Historical name for the counter — kept for external test binaries
/// that predate the feature-split. Returns the same underlying
/// number regardless of which backend is running.
pub fn boa_context_created_count() -> u64 {
    CONTEXT_CREATED_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Task 037 — cheap scan for `${...}` or whole-string `$=...=$`
/// expressions anywhere in the value tree. When this returns
/// `false`, `evaluate()` skips the JS engine entirely and returns
/// the input unchanged. Conservative: any `${` substring triggers
/// the slow path, even if the braces aren't balanced (the slow path
/// handles that correctly).
pub(crate) fn has_expressions(v: &Value) -> bool {
    match v {
        Value::String(s) => string_has_expressions(s),
        Value::Object(m) => m.values().any(has_expressions),
        Value::Array(a) => a.iter().any(has_expressions),
        _ => false,
    }
}

fn string_has_expressions(s: &str) -> bool {
    if s.contains("${") {
        return true;
    }
    // Whole-string `$= expr =` line pattern. The regex is
    // `\$=(.+)=$` where `$` anchors to end-of-string — so the
    // closing delimiter is a single `=`, NOT `=$`. Mirror the exact
    // regex check to avoid semantic drift with the slow path. Cheap
    // pre-check first (most strings don't start with `$=`).
    if !s.starts_with("$=") {
        return false;
    }
    LINE_PATTERN
        .captures(s)
        .and_then(|c| c.get(0))
        .map(|m| m.as_str() == s)
        .unwrap_or(false)
}

/// Find every balanced `${...}` segment in `s`. Returns (start, end,
/// inner) where `start..end` covers the whole `${...}` and `inner` is
/// the script body between braces. Properly nests on inner `{...}`
/// (JS object literals) and skips `${` inside string literals.
pub(crate) fn find_script_segments(s: &str) -> Vec<(usize, usize, String)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < n {
        if bytes[i] == b'$' && bytes[i + 1] == b'{' {
            let start = i;
            let mut depth = 1i32;
            let mut j = i + 2;
            let mut in_str: Option<u8> = None;
            let mut escape = false;
            while j < n {
                let b = bytes[j];
                if let Some(q) = in_str {
                    if escape {
                        escape = false;
                    } else if b == b'\\' {
                        escape = true;
                    } else if b == q {
                        in_str = None;
                    }
                } else {
                    match b {
                        b'"' | b'\'' | b'`' => in_str = Some(b),
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                let inner = s[i + 2..j].to_string();
                                out.push((start, j + 1, inner));
                                i = j + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            if depth != 0 {
                i = start + 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

// ── Limits (engine-agnostic) ──────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct ScriptLimits {
    pub max_loop_iterations: u64,
    pub max_stack_size: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            max_loop_iterations: 1_000_000,
            max_stack_size: 400,
        }
    }
}

pub(crate) static DEFAULT_LIMITS: once_cell::sync::OnceCell<ScriptLimits> =
    once_cell::sync::OnceCell::new();

/// Install process-wide default limits. Called once at boot from
/// `main` with the operator's `scripting` config. Subsequent calls
/// are ignored — the intent is a boot-time contract, not a runtime
/// knob.
pub fn install_default_limits(limits: ScriptLimits) {
    let _ = DEFAULT_LIMITS.set(limits);
}

// ── Backend selection ────────────────────────────────────────────

#[cfg(feature = "scripting-boa")]
pub mod boa;
#[cfg(feature = "scripting-boa")]
pub use boa::BoaScriptEngine as ScriptEngine;

#[cfg(feature = "scripting-quickjs")]
pub mod quickjs;
#[cfg(feature = "scripting-quickjs")]
pub use quickjs::QuickJsScriptEngine as ScriptEngine;

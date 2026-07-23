//! Task 045 — pre-parsed expression registry.
//!
//! Walks the loaded DSL tree at boot, extracts every `${...}` and
//! whole-string `$=...=` expression source, assigns each a stable
//! monotonic id, and exposes the resulting `ExpressionRegistry` for
//! the scripting backend to consume.
//!
//! On the QuickJS backend, session init bulk-compiles every
//! registered expression into `globalThis.__fn_<id>` at once. Each
//! subsequent `evaluate()` in the session just invokes `__fn_<id>()`
//! (a two-byte-plus-digit string that parses in microseconds vs the
//! multi-hundred-µs parse cost of a real expression).
//!
//! The Boa backend ignores the registry — Boa contexts are per-eval
//! and per-request, so precompilation has nowhere durable to live.
//! On Boa this module compiles to a no-op registry construction.

use super::{find_script_segments, LINE_PATTERN};
use crate::dsl::loader::{GuardDsls, HttpDsls};
use crate::dsl::Dsl;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// A frozen (post-boot) view of every JS expression that appears
/// anywhere in the loaded DSL tree. Cheap to clone via `Arc`.
///
/// Iteration order is insertion order via `IndexMap<String, u64>`
/// — matters for the QuickJS bulk-compile (each function needs a
/// stable id and we emit them in a single ordered eval).
#[derive(Debug, Clone, Default)]
pub struct ExpressionRegistry {
    inner: Arc<ExpressionRegistryInner>,
}

#[derive(Debug, Default)]
struct ExpressionRegistryInner {
    /// Expression source → id. Two identical source strings share
    /// one id (and one compiled function) — that's the whole point.
    by_source: HashMap<String, u64>,
    /// Ordered list of (id, source) for bulk-compile emission.
    /// Same content as `by_source` but with deterministic iteration.
    ordered: Vec<(u64, String)>,
}

impl ExpressionRegistry {
    /// Convenience: build from HTTP DSLs and guards in one call. For
    /// callers that also have trigger DSLs to scan (main.rs at boot),
    /// use `Builder::new()` and add all of them, then `.freeze()`.
    pub fn build_from(http: &HttpDsls, guards: &GuardDsls) -> Self {
        let mut b = Builder::new();
        b.add_http(http);
        b.add_guards(guards);
        b.freeze()
    }

    /// Number of unique expressions registered. Diagnostic.
    pub fn len(&self) -> usize {
        self.inner.ordered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.ordered.is_empty()
    }

    /// Look up the id assigned to `expr_source`. Returns `None`
    /// for expressions that weren't seen at DSL load — e.g. dynamic
    /// script sources constructed at runtime (rare; only inside
    /// `execute_js` on a fabricated string). Callers should fall
    /// back to raw eval for unknown expressions.
    pub fn id_for(&self, expr_source: &str) -> Option<u64> {
        self.inner.by_source.get(expr_source).copied()
    }

    /// Iterate (id, source) for bulk-compile emission. Ordered by
    /// id so callers can rely on deterministic function names.
    pub fn ordered(&self) -> impl Iterator<Item = (u64, &str)> {
        self.inner.ordered.iter().map(|(id, s)| (*id, s.as_str()))
    }
}

/// Incremental builder. Add DSLs from multiple sources
/// (HTTP routes, guards, triggers), then `freeze()` into an
/// immutable, `Arc`-shared `ExpressionRegistry`.
#[derive(Default)]
pub struct Builder {
    sources: Vec<String>,
    seen: HashMap<String, u64>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_http(&mut self, http: &HttpDsls) {
        for methods in http.values() {
            for paths in methods.values() {
                for dsl in paths.values() {
                    self.add_dsl(dsl);
                }
            }
        }
    }

    pub fn add_guards(&mut self, guards: &GuardDsls) {
        for guards_by_key in guards.values() {
            for dsl in guards_by_key.values() {
                self.add_dsl(dsl);
            }
        }
    }

    pub fn add_trigger_dsls(&mut self, triggers: &crate::dsl::loader::TriggerDsls) {
        for channels in triggers.values() {
            for dsl in channels.values() {
                self.add_dsl(dsl);
            }
        }
    }

    pub fn add_dsl(&mut self, dsl: &Dsl) {
        // Serialise the DSL to JSON so we don't have to know each
        // step type's individual field layout. Every string in the
        // serialised tree is a candidate for expression extraction.
        let json = match serde_json::to_value(dsl) {
            Ok(v) => v,
            Err(_) => return,
        };
        walk_json(&json, &mut self.seen, &mut self.sources);
    }

    pub fn freeze(self) -> ExpressionRegistry {
        let ordered: Vec<(u64, String)> = self
            .sources
            .into_iter()
            .enumerate()
            .map(|(i, s)| (i as u64, s))
            .collect();
        let by_source: HashMap<String, u64> =
            ordered.iter().map(|(id, s)| (s.clone(), *id)).collect();
        ExpressionRegistry {
            inner: Arc::new(ExpressionRegistryInner { by_source, ordered }),
        }
    }
}

fn walk_json(v: &Value, seen: &mut HashMap<String, u64>, sources: &mut Vec<String>) {
    match v {
        Value::String(s) => scan_string(s, seen, sources),
        Value::Object(m) => {
            for v in m.values() {
                walk_json(v, seen, sources);
            }
        }
        Value::Array(a) => {
            for v in a {
                walk_json(v, seen, sources);
            }
        }
        _ => {}
    }
}

fn scan_string(s: &str, seen: &mut HashMap<String, u64>, sources: &mut Vec<String>) {
    // Every balanced `${...}` segment in the string.
    for (_, _, inner) in find_script_segments(s) {
        register(&inner, seen, sources);
    }
    // Whole-string `$=...=` line pattern.
    if s.starts_with("$=") {
        if let Some(caps) = LINE_PATTERN.captures(s) {
            if caps.get(0).unwrap().as_str() == s {
                register(&caps[1], seen, sources);
            }
        }
    }
}

fn register(expr: &str, seen: &mut HashMap<String, u64>, sources: &mut Vec<String>) {
    if !seen.contains_key(expr) {
        let id = sources.len() as u64;
        sources.push(expr.to_string());
        seen.insert(expr.to_string(), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg_from_strings(strings: &[&str]) -> Vec<(u64, String)> {
        let mut seen = HashMap::new();
        let mut sources = Vec::new();
        for s in strings {
            scan_string(s, &mut seen, &mut sources);
        }
        sources
            .into_iter()
            .enumerate()
            .map(|(i, s)| (i as u64, s))
            .collect()
    }

    #[test]
    fn extracts_simple_dollar_brace() {
        let got = reg_from_strings(&["hello ${x} world"]);
        assert_eq!(got, vec![(0, "x".to_string())]);
    }

    #[test]
    fn extracts_multiple_in_one_string() {
        let got = reg_from_strings(&["${a} + ${b}"]);
        assert_eq!(got, vec![(0, "a".to_string()), (1, "b".to_string())]);
    }

    #[test]
    fn deduplicates_identical_sources() {
        let got = reg_from_strings(&["${x + 1}", "static text", "${x + 1}"]);
        assert_eq!(got, vec![(0, "x + 1".to_string())]);
    }

    #[test]
    fn extracts_line_pattern() {
        let got = reg_from_strings(&["$=a + b="]);
        assert_eq!(got, vec![(0, "a + b".to_string())]);
    }

    #[test]
    fn ignores_no_expression_strings() {
        let got = reg_from_strings(&["plain text", "no dollars here", ""]);
        assert!(got.is_empty());
    }

    #[test]
    fn nested_braces_captured_correctly() {
        // `${({ok: true})}` — object literal inside expression
        let got = reg_from_strings(&["${({ok: true})}"]);
        assert_eq!(got, vec![(0, "({ok: true})".to_string())]);
    }
}

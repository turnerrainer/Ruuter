//! Guard-key audit — resolve which guard KEYS gate a given DSL, in
//! the order they'd execute (outermost first). Single source of truth
//! for guard-matching semantics: same prefix/exact-match rules (#41),
//! same project-level (#39) outermost-first ordering, same
//! `override_ancestors` short-circuit, same `guards.mode` handling.
//!
//! Two consumers:
//!
//! - `DslRouter::applicable_guards` (per-request, hot path) —
//!   calls `guard_keys_for_dsl`, then looks up each key's Dsl body
//!   for execution.
//! - `dsl-lint --require-guard` + `GET /_/unguarded` (audit paths,
//!   issue #45) — call `audit_all_routes` and only care about the
//!   keys, not the Dsl bodies. No cloning of Dsls.
//!
//! Keeping the matching logic here means all three code paths cannot
//! drift. If the semantics change (e.g. a fourth guard convention
//! lands), edit ONE function.

use crate::config::GuardMode;
use crate::dsl::loader::{GuardDsls, HttpDsls, PROJECT_GUARD_KEY};
use crate::dsl::Dsl;

/// Per-route audit record. `guards` is empty when the route has no
/// applicable guard — the signal the audit callers key on.
#[derive(Debug, Clone)]
pub struct RouteAudit {
    pub project: String,
    pub method: String,
    /// Route path relative to the method prefix (e.g. `admin/users`
    /// for a DSL keyed `POST/admin/users`).
    pub path: String,
    /// Full DSL key as stored in `LoadedProjects.http` — includes the
    /// method prefix (e.g. `POST/admin/users`). Useful when joining
    /// back against other maps.
    pub dsl_key: String,
    /// Ordered outer-first: project-level guard (if any) → method-
    /// scoped ancestors → nearest ancestor. Empty when the route is
    /// unguarded.
    pub guards: Vec<String>,
}

impl RouteAudit {
    pub fn is_unguarded(&self) -> bool {
        self.guards.is_empty()
    }
}

/// Return the guard keys that gate `dsl_key` under `project`, in the
/// order they'd execute (outermost first).
///
/// Semantics mirror `DslRouter::applicable_guards`:
///
/// - **Project-level guard** (`PROJECT_GUARD_KEY`, issue #39) — always
///   the outermost, prepended regardless of `mode`.
/// - **Method-scoped guards** — match by prefix (`<guard_key>/*`) OR
///   exact-match on the same key (issue #41 fix). Sorted outer-first.
/// - **`GuardMode::Stack`** (default) — every method-scoped ancestor
///   applies, outer-first.
/// - **`GuardMode::ClosestOnly`** — only the innermost method-scoped
///   ancestor applies (Java `DslService.getGuard` parity).
/// - **`declaration.override_ancestors: true`** on a method-scoped
///   guard replaces every ancestor (most-specific wins if multiple).
///   The project-level guard is dropped by the override too — the
///   escape hatch replaces every outer guard, project-level included.
pub fn guard_keys_for_dsl(
    project: &str,
    dsl_key: &str,
    guards: &GuardDsls,
    mode: GuardMode,
) -> Vec<String> {
    let Some(project_guards) = guards.get(project) else {
        return Vec::new();
    };

    let mut matches: Vec<(usize, String)> = Vec::new();
    let mut project_guard: Option<String> = None;
    for guard_key in project_guards.keys() {
        if guard_key == PROJECT_GUARD_KEY {
            project_guard = Some(guard_key.clone());
            continue;
        }
        let prefix_with_slash = format!("{}/", guard_key);
        if dsl_key == guard_key.as_str() || dsl_key.starts_with(&prefix_with_slash) {
            matches.push((guard_key.len(), guard_key.clone()));
        }
    }
    matches.sort_by_key(|(len, _)| *len);

    let has_override = matches
        .iter()
        .any(|(_, k)| is_override_guard(project_guards.get(k)));
    if has_override {
        let longest_override = matches
            .iter()
            .filter(|(_, k)| is_override_guard(project_guards.get(k)))
            .max_by_key(|(len, _)| *len)
            .map(|(_, k)| k.clone());
        return longest_override.into_iter().collect();
    }

    let method_scoped: Vec<String> = match mode {
        GuardMode::Stack => matches.into_iter().map(|(_, k)| k).collect(),
        GuardMode::ClosestOnly => matches
            .into_iter()
            .last()
            .map(|(_, k)| vec![k])
            .unwrap_or_default(),
    };

    let mut out = Vec::with_capacity(1 + method_scoped.len());
    if let Some(k) = project_guard {
        out.push(k);
    }
    out.extend(method_scoped);
    out
}

/// Walk every HTTP DSL in the loaded tree and emit a `RouteAudit`
/// per route. Deterministically sorted by `(project, method, path)`
/// so `dsl-lint --require-guard` and `GET /_/unguarded` produce
/// stable output.
///
/// Non-HTTP methods (`WS/`) are skipped — the guard chain fires on
/// the HTTP `execute_dsl` path only, so WS routes aren't gated by
/// guards today (see `book/src/dsl/guards.md`); including them in
/// this audit would report false "unguarded" positives for handlers
/// that aren't even in scope for the guard mechanism.
pub fn audit_all_routes(http: &HttpDsls, guards: &GuardDsls, mode: GuardMode) -> Vec<RouteAudit> {
    let mut out = Vec::new();
    for (project, methods) in http {
        for (method, dsls) in methods {
            if !is_http_method(method) {
                continue;
            }
            for dsl_key in dsls.keys() {
                let guards = guard_keys_for_dsl(project, dsl_key, guards, mode);
                // dsl_key is `<METHOD>/<path>`; strip the method prefix
                // to expose the path readers already recognise from
                // URL shape.
                let path = dsl_key
                    .strip_prefix(&format!("{}/", method))
                    .unwrap_or(dsl_key)
                    .to_string();
                out.push(RouteAudit {
                    project: project.clone(),
                    method: method.clone(),
                    path,
                    dsl_key: dsl_key.clone(),
                    guards,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then_with(|| a.method.cmp(&b.method))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

fn is_override_guard(dsl: Option<&Dsl>) -> bool {
    dsl.and_then(|d| d.declaration.as_ref())
        .and_then(|d| d.override_ancestors)
        .unwrap_or(false)
}

fn is_http_method(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "HEAD"
    )
}

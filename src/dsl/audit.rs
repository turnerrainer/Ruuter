//! Issue #146 — self-audit engine for production-readiness gaps in
//! the loaded DSL tree.
//!
//! Called by two surfaces:
//! - The admin-gated HTTP endpoint `GET /_/audit/dsl` — runs against
//!   the currently loaded tree so hot-reload is reflected.
//! - `dsl-lint` — runs against the filesystem at build time.
//!
//! Both surfaces call `audit_tree(&loaded_http)` which returns a flat
//! `Vec<Finding>` the caller formats per its own output shape (JSON
//! for the endpoint, human-readable or `--json` for the lint).
//!
//! Design notes:
//!
//! - Flat findings list (not nested by project/DSL) so dashboards can
//!   `group_by` on any field.
//! - `code` is a stable string key — downstream tooling pins on these;
//!   adding a new one is minor-bump surface, renaming an existing one
//!   is breaking.
//! - Severities: `Error` = unambiguously broken, operator must fix;
//!   `Warning` = drift / posture gap, operator should fix; `Info` =
//!   soft signal / nice-to-have.
//! - V1 ships Categories A, B, C (simple), and E from issue #146.
//!   Category C's `unchecked_dereference`, Category D (cross-DSL
//!   guard contract), and Category F (OpenAPI vs DSL mismatch) are
//!   deferred to a follow-up — those need expression AST walking and
//!   cross-DSL reasoning.

use crate::dsl::loader::HttpDsls;
use crate::dsl::{Dsl, DslField};
use serde::Serialize;
use std::collections::HashSet;

/// Severity for a finding. Dashboards typically filter on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Unambiguously broken; operator must fix.
    Error,
    /// Drift or posture gap; operator should fix.
    Warning,
    /// Soft signal; nice-to-have.
    Info,
}

/// One audit finding — a single `(project, dsl, code)` triple with a
/// human-readable message and optional field list.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub project: String,
    pub dsl: String,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}

/// Audit every HTTP DSL in the loaded tree. Returns findings sorted
/// by `(project, dsl, code)` so diffs across polls are meaningful.
///
/// Does NOT audit triggers or WS DSLs; those don't have the same
/// declaration-driven security contract and would need their own
/// check catalogue.
pub fn audit_tree(http: &HttpDsls) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (project, methods) in http {
        for (method, dsls) in methods {
            for (key, dsl) in dsls {
                audit_dsl(project, method, key, dsl, &mut findings);
            }
        }
    }
    findings.sort_by(|a, b| {
        a.project
            .cmp(&b.project)
            .then_with(|| a.dsl.cmp(&b.dsl))
            .then_with(|| a.code.cmp(b.code))
    });
    findings
}

/// Audit a single DSL. Pushed-into `out` so the aggregator can
/// deduplicate / sort / count across the whole tree at the top level.
fn audit_dsl(project: &str, _method: &str, dsl_key: &str, dsl: &Dsl, out: &mut Vec<Finding>) {
    // Category A — Declaration completeness.
    let Some(decl) = &dsl.declaration else {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Warning,
            code: "declaration.missing",
            message: "No `declaration:` block. OpenAPI operation gets \
                      auto-description; no request/response schemas emitted; \
                      allowlist filtering is a no-op."
                .to_string(),
            fields: Vec::new(),
        });
        return;
    };

    if decl
        .description
        .as_ref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Info,
            code: "declaration.description_missing",
            message: "No `description:`. OpenAPI operation falls back to \
                      `Auto-generated from DSL ...`."
                .to_string(),
            fields: Vec::new(),
        });
    }

    if decl.returns.is_none() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Info,
            code: "declaration.returns_missing",
            message: "No `returns:` response schema. OpenAPI 200 response \
                      falls back to `{type:object, additionalProperties:true}`; \
                      client codegen cannot type the response."
                .to_string(),
            fields: Vec::new(),
        });
    }

    // Legacy flat allowlist markers (no field metadata available).
    let legacy_flat: Vec<String> = {
        let mut v = Vec::new();
        if decl.allowed_body.is_some() {
            v.push("allowed_body".to_string());
        }
        if decl.allowed_params.is_some() {
            v.push("allowed_params".to_string());
        }
        if decl.allowed_header.is_some() {
            v.push("allowed_header".to_string());
        }
        v
    };
    if !legacy_flat.is_empty() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Info,
            code: "declaration.legacy_flat_allowlist",
            message: "Using legacy flat allowlist form(s); no field-level \
                      `type:` / `required:` / `description:` metadata is \
                      available. Prefer structured `allowlist.*`."
                .to_string(),
            fields: legacy_flat,
        });
    }

    // Category A continued — missing type: on structured body fields.
    if let Some(allowlist) = &decl.allowlist {
        if let Some(body) = &allowlist.body {
            let missing: Vec<String> = body
                .iter()
                .filter(|f| f.field_type.is_none())
                .map(|f| f.field.clone())
                .collect();
            if !missing.is_empty() {
                out.push(Finding {
                    project: project.to_string(),
                    dsl: dsl_key.to_string(),
                    severity: Severity::Info,
                    code: "declaration.body.type_missing",
                    message: "One or more body fields lack a `type:` hint. \
                              OpenAPI emits generic `string`; the issue #75 \
                              type coercion has no referent."
                        .to_string(),
                    fields: missing,
                });
            }
        }
    }

    // Category B — Allowlist drift.
    //
    // "Referenced" is detected by scanning the serialized DSL body
    // for `${incoming.<section>.<field>}` or `${incoming.<section>['<field>']}`
    // or `${incoming.<section>["<field>"]}`. The DSL steps are serde
    // JSON-serialisable, so render once and run cheap regex-free
    // substring checks per field name. False positives possible if
    // a step body uses the field name as a literal unrelated string;
    // the audit is a hint, not a hard error.
    let rendered = render_dsl_for_scan(dsl);
    let decl_body_fields: Vec<String> = decl
        .allowlist
        .as_ref()
        .and_then(|a| a.body.as_ref())
        .map(|v| v.iter().map(|f| f.field.clone()).collect())
        .unwrap_or_default();
    let decl_param_fields: Vec<String> = decl
        .allowlist
        .as_ref()
        .and_then(|a| a.params.as_ref())
        .map(|v| v.iter().map(|f| f.field.clone()).collect())
        .unwrap_or_default();
    let decl_header_fields: Vec<String> = decl
        .allowlist
        .as_ref()
        .and_then(|a| a.headers.as_ref())
        .map(|v| v.iter().map(|f| f.field.clone()).collect())
        .unwrap_or_default();

    // Over-declared: declared field never referenced in the DSL body.
    emit_over_declared(&rendered, "body", &decl_body_fields, project, dsl_key, out);
    emit_over_declared(
        &rendered,
        "params",
        &decl_param_fields,
        project,
        dsl_key,
        out,
    );
    emit_over_declared_headers(&rendered, &decl_header_fields, project, dsl_key, out);

    // Under-declared: DSL references a field that isn't declared.
    emit_under_declared(&rendered, "body", &decl_body_fields, project, dsl_key, out);
    emit_under_declared(
        &rendered,
        "params",
        &decl_param_fields,
        project,
        dsl_key,
        out,
    );
    emit_under_declared_headers(&rendered, &decl_header_fields, project, dsl_key, out);

    // Category C — security posture: required_but_unused.
    if let Some(allowlist) = &decl.allowlist {
        if let Some(body) = &allowlist.body {
            let mut unused_required: Vec<String> = Vec::new();
            for f in body {
                if f.required == Some(true) && !is_body_field_referenced(&rendered, &f.field) {
                    unused_required.push(f.field.clone());
                }
            }
            if !unused_required.is_empty() {
                out.push(Finding {
                    project: project.to_string(),
                    dsl: dsl_key.to_string(),
                    severity: Severity::Warning,
                    code: "declaration.required_but_unused",
                    message: "Field(s) marked `required: true` but never \
                              referenced in the DSL body. Clients are forced \
                              to send dead data."
                        .to_string(),
                    fields: unused_required,
                });
            }
        }
    }

    // Category E — Internal + reachability (ties to issue #143).
    if decl.internal.is_none() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Info,
            code: "declaration.internal_missing",
            message: "No `declaration.internal` set. Resolves via operator \
                      fallback (`declarations.default_internal`; issue #143). \
                      Set explicitly to pin the posture or add \
                      `dsl-lint --require-internal-explicit` to CI."
                .to_string(),
            fields: Vec::new(),
        });
    }
}

/// Render the DSL steps (minus the declaration block) to a string for
/// cheap substring scans. Serde JSON gives us a stable textual form
/// that reflects every templating expression the StepEngine will see
/// at runtime. The declaration itself is intentionally excluded —
/// otherwise a field name declared in `allowlist.body` would always
/// "find itself" in the serialization and over-declared could never
/// fire.
fn render_dsl_for_scan(dsl: &Dsl) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, step) in &dsl.steps {
        if matches!(step, crate::steps::DslStep::Declaration(_)) {
            continue;
        }
        if let Ok(s) = serde_json::to_string(step) {
            parts.push(format!("{}:{}", name, s));
        }
    }
    parts.join("\n")
}

fn is_body_field_referenced(rendered: &str, field: &str) -> bool {
    is_field_referenced(rendered, "body", field, false)
}

/// `${incoming.<section>.<field>}` OR `${incoming.<section>['<field>']}` /
/// `${incoming.<section>["<field>"]}`. For headers only, match
/// case-insensitively (HTTP header names are case-insensitive).
fn is_field_referenced(rendered: &str, section: &str, field: &str, case_insensitive: bool) -> bool {
    // Dot form: `incoming.<section>.<field>`
    let dot = format!("incoming.{}.{}", section, field);
    // Bracket forms with either quote style.
    let br_single = format!("incoming.{}['{}']", section, field);
    let br_double = format!("incoming.{}[\"{}\"]", section, field);
    if case_insensitive {
        let haystack = rendered.to_ascii_lowercase();
        let d = dot.to_ascii_lowercase();
        let s = br_single.to_ascii_lowercase();
        let dd = br_double.to_ascii_lowercase();
        haystack.contains(&d) || haystack.contains(&s) || haystack.contains(&dd)
    } else {
        rendered.contains(&dot) || rendered.contains(&br_single) || rendered.contains(&br_double)
    }
}

fn emit_over_declared(
    rendered: &str,
    section: &str,
    declared: &[String],
    project: &str,
    dsl_key: &str,
    out: &mut Vec<Finding>,
) {
    let unused: Vec<String> = declared
        .iter()
        .filter(|f| !is_field_referenced(rendered, section, f, false))
        .cloned()
        .collect();
    if !unused.is_empty() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Warning,
            code: section_code(section, "over_declared"),
            message: format!(
                "Field(s) declared in allowlist.{} are never referenced via \
                 ${{incoming.{}.<field>}} in the DSL body. Clients forced to \
                 send dead data; OpenAPI advertises fields that do nothing.",
                section, section
            ),
            fields: unused,
        });
    }
}

fn emit_over_declared_headers(
    rendered: &str,
    declared: &[String],
    project: &str,
    dsl_key: &str,
    out: &mut Vec<Finding>,
) {
    // Framework-level headers OpenAPI needs regardless — skip them
    // in the "unused" check so a route that only cares about an
    // Authorization header via a guard isn't flagged for not reading
    // `incoming.headers.authorization` itself.
    let safe: HashSet<&'static str> =
        ["authorization", "traceparent", "content-type", "x-trace-id"]
            .into_iter()
            .collect();
    let unused: Vec<String> = declared
        .iter()
        .filter(|f| {
            !safe.contains(f.to_ascii_lowercase().as_str())
                && !is_field_referenced(rendered, "headers", f, true)
        })
        .cloned()
        .collect();
    if !unused.is_empty() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Warning,
            code: "declaration.headers.over_declared",
            message: "Header(s) declared in allowlist.headers are never \
                      referenced via ${incoming.headers.<name>} in the DSL \
                      body. Framework-level headers (authorization, \
                      traceparent, content-type, x-trace-id) are excluded \
                      from this check."
                .to_string(),
            fields: unused,
        });
    }
}

fn emit_under_declared(
    rendered: &str,
    section: &str,
    declared: &[String],
    project: &str,
    dsl_key: &str,
    out: &mut Vec<Finding>,
) {
    let references = extract_references(rendered, section, false);
    let decl_set: HashSet<&String> = declared.iter().collect();
    let missing: Vec<String> = references
        .iter()
        .filter(|r| !decl_set.contains(r))
        .cloned()
        .collect();
    if !missing.is_empty() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Warning,
            code: section_code(section, "under_declared"),
            message: format!(
                "DSL references ${{incoming.{}.<field>}} for field(s) not \
                 declared in allowlist.{}. Under default filter the field \
                 is silently stripped before the step sees it; under \
                 `strict:` the request 400s. OpenAPI omits the field, so \
                 clients have no reason to send it.",
                section, section
            ),
            fields: missing,
        });
    }
}

fn emit_under_declared_headers(
    rendered: &str,
    declared: &[String],
    project: &str,
    dsl_key: &str,
    out: &mut Vec<Finding>,
) {
    let references = extract_references(rendered, "headers", true);
    let decl_lower: HashSet<String> = declared.iter().map(|s| s.to_ascii_lowercase()).collect();
    let safe: HashSet<&'static str> =
        ["authorization", "traceparent", "content-type", "x-trace-id"]
            .into_iter()
            .collect();
    let missing: Vec<String> = references
        .iter()
        .filter(|r| {
            !decl_lower.contains(&r.to_ascii_lowercase())
                && !safe.contains(r.to_ascii_lowercase().as_str())
        })
        .cloned()
        .collect();
    if !missing.is_empty() {
        out.push(Finding {
            project: project.to_string(),
            dsl: dsl_key.to_string(),
            severity: Severity::Warning,
            code: "declaration.headers.under_declared",
            message: "DSL references ${incoming.headers.<name>} for \
                      header(s) not declared in allowlist.headers. \
                      Framework-level headers are excluded."
                .to_string(),
            fields: missing,
        });
    }
}

/// Extract `incoming.<section>.<field>` references from the rendered
/// body. Returns deduplicated field names. Only matches identifier-
/// style names (letters, digits, underscore, dash); bracket-indexed
/// forms with arbitrary expressions are not supported — they would
/// require a real parser.
fn extract_references(rendered: &str, section: &str, _headers: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let prefix = format!("incoming.{}.", section);
    let mut rest = rendered;
    while let Some(pos) = rest.find(&prefix) {
        let after = &rest[pos + prefix.len()..];
        let end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
            .unwrap_or(after.len());
        if end > 0 {
            let name = &after[..end];
            if !out.iter().any(|s| s == name) {
                out.push(name.to_string());
            }
        }
        rest = &rest[pos + prefix.len() + end..];
    }
    out
}

fn section_code(section: &str, kind: &str) -> &'static str {
    // Static strings only — Finding::code is &'static str so callers
    // can key on string equality cheaply.
    match (section, kind) {
        ("body", "over_declared") => "declaration.body.over_declared",
        ("body", "under_declared") => "declaration.body.under_declared",
        ("params", "over_declared") => "declaration.params.over_declared",
        ("params", "under_declared") => "declaration.params.under_declared",
        // headers handled separately for case-insensitivity + safe set
        _ => unreachable!("section_code called for unsupported (section, kind)"),
    }
}

/// Suppresses the unused-type warning on `DslField` when audit.rs is
/// the only consumer. (Keeps the module compiling in isolation.)
#[allow(dead_code)]
fn _ensure_dslfield_used(_: &DslField) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::dsl::loader::DslLoader;
    use std::collections::HashMap;
    use std::fs;

    fn fresh_root(name: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("ruuter-146-audit-{}-{}", name, ns));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(root: &std::path::Path, rel: &str, body: &str) {
        let full = root.join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, body).unwrap();
    }

    fn load(root: &std::path::Path) -> crate::dsl::loader::LoadedProjects {
        let mut cfg = AppConfig::default();
        cfg.config_path = root.to_path_buf();
        DslLoader::new(cfg, HashMap::new())
            .load_everything()
            .unwrap()
    }

    #[test]
    fn declaration_missing_fires() {
        let root = fresh_root("decl-missing");
        write(
            &root,
            "svc/GET/no-decl.yml",
            "r:\n  return: ok\n  next: end\n",
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        assert!(
            findings.iter().any(|f| f.code == "declaration.missing"),
            "expected declaration.missing; got {:?}",
            findings.iter().map(|f| f.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn description_missing_fires_when_description_absent() {
        let root = fresh_root("desc-missing");
        write(
            &root,
            "svc/GET/no-desc.yml",
            "declaration:\n  internal: false\nr:\n  return: ok\n  next: end\n",
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        assert!(findings
            .iter()
            .any(|f| f.code == "declaration.description_missing"));
    }

    #[test]
    fn over_declared_body_fires_for_unreferenced_field() {
        let root = fresh_root("over-body");
        write(
            &root,
            "svc/POST/users.yml",
            concat!(
                "declaration:\n",
                "  internal: false\n",
                "  description: 'u'\n",
                "  returns: []\n",
                "  allowlist:\n",
                "    body:\n",
                "      - field: name\n",
                "      - field: unused_dead\n",
                "r:\n",
                "  return: '${incoming.body.name}'\n",
                "  next: end\n",
            ),
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        let over = findings
            .iter()
            .find(|f| f.code == "declaration.body.over_declared");
        assert!(over.is_some(), "want over_declared; got {:?}", findings);
        assert_eq!(over.unwrap().fields, vec!["unused_dead".to_string()]);
    }

    #[test]
    fn under_declared_body_fires_for_undeclared_reference() {
        let root = fresh_root("under-body");
        write(
            &root,
            "svc/POST/users.yml",
            concat!(
                "declaration:\n",
                "  internal: false\n",
                "  description: 'u'\n",
                "  returns: []\n",
                "  allowlist:\n",
                "    body:\n",
                "      - field: name\n",
                "r:\n",
                "  return:\n",
                "    greet: '${incoming.body.name}'\n",
                "    dob: '${incoming.body.dob}'\n",
                "  next: end\n",
            ),
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        let under = findings
            .iter()
            .find(|f| f.code == "declaration.body.under_declared");
        assert!(under.is_some(), "want under_declared; got {:?}", findings);
        assert_eq!(under.unwrap().fields, vec!["dob".to_string()]);
    }

    #[test]
    fn internal_missing_fires_when_absent() {
        let root = fresh_root("internal-missing");
        write(
            &root,
            "svc/GET/pub.yml",
            "declaration:\n  description: 'p'\nr:\n  return: ok\n  next: end\n",
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        assert!(findings
            .iter()
            .any(|f| f.code == "declaration.internal_missing"));
    }

    #[test]
    fn internal_set_suppresses_internal_missing() {
        let root = fresh_root("internal-set");
        write(
            &root,
            "svc/GET/pub.yml",
            "declaration:\n  description: 'p'\n  internal: false\nr:\n  return: ok\n  next: end\n",
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        assert!(!findings
            .iter()
            .any(|f| f.code == "declaration.internal_missing"));
    }

    #[test]
    fn required_but_unused_body_fires() {
        let root = fresh_root("required-unused");
        write(
            &root,
            "svc/POST/users.yml",
            concat!(
                "declaration:\n",
                "  internal: false\n",
                "  description: 'u'\n",
                "  allowlist:\n",
                "    body:\n",
                "      - field: name\n",
                "      - field: dead\n",
                "        required: true\n",
                "r:\n",
                "  return: '${incoming.body.name}'\n",
                "  next: end\n",
            ),
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        let req = findings
            .iter()
            .find(|f| f.code == "declaration.required_but_unused");
        assert!(
            req.is_some(),
            "want required_but_unused; got {:?}",
            findings
        );
        assert_eq!(req.unwrap().fields, vec!["dead".to_string()]);
    }

    #[test]
    fn legacy_flat_allowlist_flagged() {
        let root = fresh_root("legacy-flat");
        write(
            &root,
            "svc/POST/users.yml",
            "declaration:\n  internal: false\n  description: 'u'\n  allowed_body: [name]\n\
             r:\n  return: '${incoming.body.name}'\n  next: end\n",
        );
        let tree = load(&root);
        let findings = audit_tree(&tree.http);
        assert!(findings
            .iter()
            .any(|f| f.code == "declaration.legacy_flat_allowlist"));
    }
}

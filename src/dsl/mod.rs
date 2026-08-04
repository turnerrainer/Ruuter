use crate::steps::DslStep;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub mod hot_reload;
pub mod interpolate;
pub mod loader;
pub mod parser;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dsl {
    pub steps: IndexMap<String, DslStep>,
    #[serde(skip)]
    pub declaration: Option<DeclarationStep>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeclarationStep {
    pub version: Option<String>,
    pub description: Option<String>,
    pub method: Option<String>,
    pub accepts: Option<String>,
    pub returns: Option<String>,
    pub namespace: Option<String>,
    pub allowed_body: Option<Vec<String>>,
    pub allowed_header: Option<Vec<String>>,
    pub allowed_params: Option<Vec<String>>,
    /// Audit finding 10 — Java-parity structured allowlist. When
    /// present, `allowed_body`, `allowed_header`, `allowed_params`
    /// derive from `allowlist.body`, `allowlist.headers`,
    /// `allowlist.params` (each entry is a `{field: <name>}` map).
    /// Explicit legacy flat fields still win over the structured
    /// form; use `.effective_allowed_*` accessors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowlist: Option<Allowlist>,
    /// Task 020 — when `Some(true)` on a guard DSL, this guard REPLACES
    /// all ancestor guards for the routes it protects (rather than
    /// stacking on top of them). Used when a specific endpoint has
    /// materially different privilege than its siblings — e.g. a
    /// stricter admin gate that shouldn't be additive to a folder-wide
    /// "authenticated" check.
    pub override_ancestors: Option<bool>,
    /// Audit finding 01 — Declaration steps also carry the base
    /// fields so a bare `{ reload_dsl: true, next: end }` step can
    /// trigger a reload (see parser's control-flow-only fallback).
    #[serde(flatten)]
    pub base: crate::steps::BaseStepFields,
}

/// Audit finding 10 — Java's structured `allowlist:` block. Each
/// entry is a `DslField` (currently a single `field:` string; the
/// Java version reserves room for per-field metadata like `format:`,
/// `required:`, etc.).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Allowlist {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<DslField>>,
    /// Java accepts both `headers` and `header`. Rust accepts the
    /// same aliases via serde.
    #[serde(default, alias = "header", skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<DslField>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<DslField>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DslField {
    pub field: String,
}

impl DeclarationStep {
    /// Effective body-field allowlist: legacy flat field wins; else
    /// derived from `allowlist.body`; else None (no allowlist).
    pub fn effective_allowed_body(&self) -> Option<Vec<String>> {
        self.allowed_body
            .clone()
            .or_else(|| self.allowlist.as_ref().and_then(|a| a.body.as_ref()).map(|v| v.iter().map(|f| f.field.clone()).collect()))
    }

    /// Effective header allowlist. Same precedence as body.
    pub fn effective_allowed_header(&self) -> Option<Vec<String>> {
        self.allowed_header
            .clone()
            .or_else(|| self.allowlist.as_ref().and_then(|a| a.headers.as_ref()).map(|v| v.iter().map(|f| f.field.clone()).collect()))
    }

    /// Effective query-params allowlist. Same precedence as body.
    pub fn effective_allowed_params(&self) -> Option<Vec<String>> {
        self.allowed_params
            .clone()
            .or_else(|| self.allowlist.as_ref().and_then(|a| a.params.as_ref()).map(|v| v.iter().map(|f| f.field.clone()).collect()))
    }
}

impl Dsl {
    pub fn new(steps: IndexMap<String, DslStep>) -> Self {
        let declaration = steps.values().find_map(|step| {
            if let DslStep::Declaration(decl) = step {
                Some(decl.clone())
            } else {
                None
            }
        });

        Self { steps, declaration }
    }

    pub fn get_step(&self, name: &str) -> Option<&DslStep> {
        self.steps.get(name)
    }

    pub fn step_names(&self) -> Vec<String> {
        self.steps.keys().cloned().collect()
    }
}

use crate::steps::DslStep;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod guard_audit;
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
    pub namespace: Option<String>,
    pub allowed_body: Option<Vec<String>>,
    pub allowed_header: Option<Vec<String>>,
    pub allowed_params: Option<Vec<String>>,
    /// Audit finding 10 — Java-parity structured allowlist. When
    /// present, `allowed_body`, `allowed_header`, `allowed_params`
    /// derive from `allowlist.body`, `allowlist.headers`,
    /// `allowlist.params` (each entry is a `{field: <name>}` map or
    /// a richer entry with per-field metadata; see `DslField`).
    /// Explicit legacy flat fields still win over the structured
    /// form; use `.effective_allowed_*` accessors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowlist: Option<Allowlist>,
    /// Task 070 — structured response schema. When set, OpenAPI's
    /// 200 response for this DSL emits the declared properties (with
    /// types, formats, and a required array); otherwise the spec
    /// falls back to `{"type":"object","additionalProperties":true}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<Vec<DslField>>,
    /// Task 070 — per-DSL opt-in for strict-unknown-keys posture.
    /// When `Some(true)`, the router rejects body / query / header
    /// keys not in the effective allowlist with a 400. Default
    /// `None` (Ruuter's traditional filter-and-continue posture).
    /// Only meaningful when at least one allowlist is declared —
    /// with no allowlist, "unknown" isn't defined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
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
/// entry is a `DslField` (either a bare `{field: <name>}` map or a
/// richer entry with per-field type metadata; see `DslField`).
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

/// Task 070 — per-field metadata used by allowlist entries AND
/// response schemas. Backwards-compat: a bare `{field: userName}`
/// still parses (all extended fields default to `None`). Richer
/// entries opt in per-field:
///
/// ```yaml
/// - field: userName
///   type: string
///   required: true
///   format: email
///   description: "Login handle."
///   default: "guest"
/// - field: tags
///   type: array
///   items:
///     field: __item__
///     type: string
/// ```
///
/// `type` values Ruuter maps to OpenAPI directly:
/// `string`, `integer`, `number`, `boolean`, `array`, `object`.
/// `format` (`email`, `uuid`, `date-time`, …) is passed through
/// verbatim. Same vocabulary as Resql task 008, so a partner
/// consuming both services' `openapi.json` sees one schema shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DslField {
    pub field: String,
    /// OpenAPI type name; passed through to `schema.type`. Absent →
    /// falls back to `string` in the generated spec.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub field_type: Option<String>,
    /// Whether the field is required. Absent → `false` for request
    /// parameters (Ruuter default); response schemas use it to
    /// populate `required: [...]` on the response body schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// OpenAPI `format` hint (e.g. `email`, `date-time`, `uuid`).
    /// Absent → not emitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Human-readable description for the OpenAPI spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Default value (any JSON literal). Emitted as `default:` on
    /// the field's schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// For `type: array` — the item schema (recursive DslField).
    /// Ignored for non-array types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<DslField>>,
}

impl DeclarationStep {
    /// Effective body-field allowlist: legacy flat field wins; else
    /// derived from `allowlist.body`; else None (no allowlist).
    pub fn effective_allowed_body(&self) -> Option<Vec<String>> {
        self.allowed_body.clone().or_else(|| {
            self.allowlist
                .as_ref()
                .and_then(|a| a.body.as_ref())
                .map(|v| v.iter().map(|f| f.field.clone()).collect())
        })
    }

    /// Effective header allowlist. Same precedence as body.
    pub fn effective_allowed_header(&self) -> Option<Vec<String>> {
        self.allowed_header.clone().or_else(|| {
            self.allowlist
                .as_ref()
                .and_then(|a| a.headers.as_ref())
                .map(|v| v.iter().map(|f| f.field.clone()).collect())
        })
    }

    /// Effective query-params allowlist. Same precedence as body.
    pub fn effective_allowed_params(&self) -> Option<Vec<String>> {
        self.allowed_params.clone().or_else(|| {
            self.allowlist
                .as_ref()
                .and_then(|a| a.params.as_ref())
                .map(|v| v.iter().map(|f| f.field.clone()).collect())
        })
    }

    /// Task 070 — whether strict-unknown-keys posture is on for
    /// this DSL. `Some(true)` → router rejects unknown body / query
    /// / header keys with a 400. Absent or `Some(false)` → traditional
    /// filter-and-continue.
    pub fn is_strict(&self) -> bool {
        self.strict.unwrap_or(false)
    }

    /// Task 070 — structured body allowlist (with per-field metadata).
    /// `None` when the DSL uses only the legacy flat `allowed_body:
    /// [name, ...]` form. Consumers that need the type / required /
    /// format hints (OpenAPI generator) prefer this over
    /// `effective_allowed_body`.
    pub fn structured_body(&self) -> Option<&[DslField]> {
        self.allowlist.as_ref().and_then(|a| a.body.as_deref())
    }

    /// Task 070 — structured params allowlist. See `structured_body`.
    pub fn structured_params(&self) -> Option<&[DslField]> {
        self.allowlist.as_ref().and_then(|a| a.params.as_deref())
    }

    /// Task 070 — structured headers allowlist. See `structured_body`.
    pub fn structured_headers(&self) -> Option<&[DslField]> {
        self.allowlist.as_ref().and_then(|a| a.headers.as_deref())
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

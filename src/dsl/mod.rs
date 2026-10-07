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

impl Dsl {
    /// Issue #143 — resolve the effective internal classification for
    /// this DSL via the three-level fallback chain. `default_internal`
    /// comes from `AppConfig.declarations.default_internal`; the
    /// framework-default `false` is applied by the caller when the
    /// config block is absent (serde provides it via `DeclarationsConfig::default`).
    ///
    /// Returns `true` when external HTTP must be denied (gate with 404
    /// at the dispatch handler); `false` when the DSL is publicly
    /// routable.
    pub fn effective_internal(&self, default_internal: bool) -> bool {
        self.declaration
            .as_ref()
            .and_then(|d| d.internal)
            .unwrap_or(default_internal)
    }
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
    /// Issue #75 — per-DSL opt-in for additive posture. When
    /// `Some(true)`, the router does NOT filter body / query / header
    /// maps down to the declared allowlist — undeclared fields pass
    /// through to `${incoming.*}` unchanged. The `required:` check
    /// still fires; OpenAPI still emits the declared schema. Use when
    /// the DSL wants the allowlist purely as documentation / OpenAPI
    /// metadata rather than as an input firewall.
    ///
    /// Mutually exclusive with `strict:`. Setting both is a parse-
    /// time error (contradictory postures — strict rejects unknown
    /// keys, additive permits them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additive: Option<bool>,
    /// Task 020 — when `Some(true)` on a guard DSL, this guard REPLACES
    /// all ancestor guards for the routes it protects (rather than
    /// stacking on top of them). Used when a specific endpoint has
    /// materially different privilege than its siblings — e.g. a
    /// stricter admin gate that shouldn't be additive to a folder-wide
    /// "authenticated" check.
    pub override_ancestors: Option<bool>,
    /// Issue #134 — pass-through proxy declaration. When set, the
    /// route becomes a streaming byte-identical HTTP proxy to the
    /// configured upstream. The DSL body MUST be empty (no action
    /// steps); the request is handled by `router::proxy` which
    /// bypasses `StepEngine` entirely. Guards still run against a
    /// header-only context (empty `incoming.body`) before any upstream
    /// connection is opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyDeclaration>,
    /// Issue #143 — when `Some(true)`, this DSL is NOT reachable via
    /// external HTTP. The dispatcher returns 404 (not 403, to avoid
    /// leaking that the route exists) for any request that didn't
    /// originate from the in-process self-call handler. `template:`
    /// sub-calls and self-call-shortcircuited `http.*` steps still
    /// reach the DSL — the self-call handler sets a request-extension
    /// marker the dispatcher checks before guard chain evaluation.
    /// `Some(false)` keeps the DSL publicly reachable (useful to
    /// opt out when the operator-level default is `true`). `None`
    /// resolves via `AppConfig.declarations.default_internal` (which
    /// itself defaults to `false` — hard-coded framework fallback so
    /// an upgrade without `ruuter.yaml` changes keeps every route
    /// public).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
    /// Audit finding 01 — Declaration steps also carry the base
    /// fields so a bare `{ reload_dsl: true, next: end }` step can
    /// trigger a reload (see parser's control-flow-only fallback).
    #[serde(flatten)]
    pub base: crate::steps::BaseStepFields,
}

/// Issue #134 — per-route pass-through proxy configuration. Attached
/// to a `DeclarationStep` via `proxy:`. All timeouts are in
/// milliseconds; `max_body_bytes` is required so proxy routes never
/// inherit an implicit cap (the global 16 MiB inbound cap applies to
/// non-proxy routes only). Defaults are tuned for AS4/eDelivery edge
/// usage; adjust per deployment.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyDeclaration {
    /// Absolute upstream URL to which every request on this route is
    /// forwarded. Interpolation (`[#CONST]`, `#{CONST}`) runs at DSL
    /// parse time via `DslParser::replace_constants` — the value at
    /// runtime is a plain URL. SSRF checks apply to this URL on every
    /// request so a compromised constants file cannot smuggle traffic
    /// to private origins.
    pub upstream: String,

    /// Whether to forward the client's request headers to the upstream
    /// (minus hop-by-hop headers per RFC 7230 §6.1). Default `true`.
    /// Set `false` to send only a minimal header set (Host rewritten
    /// to upstream, Content-Type and Content-Length preserved,
    /// everything else dropped).
    #[serde(default = "default_proxy_preserve_headers")]
    pub preserve_headers: bool,

    /// Per-route inbound body cap in bytes. Required field (no
    /// implicit cap for proxy routes). A declared Content-Length over
    /// the cap is rejected with 413 before any body is read; chunked
    /// requests are counted mid-stream and aborted on breach.
    pub max_body_bytes: u64,

    /// Maximum concurrent in-flight proxied requests on this route.
    /// 33rd request on a cap of 32 receives 503 + `Retry-After: 1`.
    /// `None` disables the cap (not recommended on public listeners;
    /// `warn_on_pass_through_proxy_defaults` fires a boot WARN).
    #[serde(default = "default_proxy_max_in_flight")]
    pub max_in_flight: Option<u32>,

    /// Idle-frame timeout on the inbound body stream, in milliseconds.
    /// Resets every time a body frame is received. Mitigates slowloris
    /// without penalising legitimate slow uploaders — a 1 Gbps upload
    /// of a 100 GiB body never trips this as long as frames keep
    /// arriving.
    #[serde(default = "default_proxy_inbound_progress_timeout_ms")]
    pub inbound_progress_timeout_ms: Option<u64>,

    /// Allowed values for the inbound `Content-Encoding` header.
    /// Default `["identity"]` — operators opt into `gzip`/`br`/`zstd`
    /// per route. A byte-identical proxy never decompresses; the
    /// allowlist exists to control which compression bombs are allowed
    /// to flow through to the upstream.
    #[serde(default = "default_proxy_allowed_encodings")]
    pub allowed_encodings: Vec<String>,

    /// End-to-end deadline for the proxied request, in milliseconds.
    /// Covers connect + request send + response receive. Default
    /// 60 000 (matches the eFTI Gate AS4 budget). Does NOT include
    /// time spent waiting for a Semaphore slot — concurrency-cap
    /// backpressure surfaces as 503 before the timeout starts.
    #[serde(default = "default_proxy_request_timeout_ms")]
    pub request_timeout_ms: Option<u64>,
}

fn default_proxy_preserve_headers() -> bool {
    true
}

fn default_proxy_max_in_flight() -> Option<u32> {
    Some(32)
}

fn default_proxy_inbound_progress_timeout_ms() -> Option<u64> {
    Some(10_000)
}

fn default_proxy_allowed_encodings() -> Vec<String> {
    vec!["identity".to_string()]
}

fn default_proxy_request_timeout_ms() -> Option<u64> {
    Some(60_000)
}

/// Issue #134 — RFC-vocabulary encoding names a proxy route is willing
/// to pass through. `identity` is special (equivalent to no
/// `Content-Encoding` header on the wire); the others are the only
/// IANA-registered HTTP content codings in common use. Unknown values
/// are a parse-time error — a typo'd `allowed_encodings: [gzipp]`
/// would otherwise silently accept nothing.
pub const KNOWN_CONTENT_ENCODINGS: &[&str] = &["identity", "gzip", "deflate", "br", "zstd"];

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
    /// Issue #75 — express "at least one of X, Y, Z must be present"
    /// contracts. Each `Vec<String>` is one alternative group; the
    /// group is satisfied when the request carries at least one of
    /// its fields. Applied per-section so headers-only, params-only,
    /// or body-only constraints stay explicit. Motivating case from
    /// the reporter: a guard that admits on `X-Api-Key` OR
    /// `X-Internal-Service-Token` couldn't declare its credential
    /// contract at all because `allowlist.headers` treated both
    /// entries as mandatory.
    ///
    /// ```yaml
    /// allowlist:
    ///   headers:
    ///     - field: x-api-key
    ///     - field: x-internal-service-token
    ///   required_one_of:
    ///     headers:
    ///       - [x-api-key, x-internal-service-token]
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_one_of: Option<RequiredOneOf>,
}

/// Issue #75 — per-section "at least one of these fields must be
/// present" groups. Each inner `Vec<String>` is one alternative
/// group; the group is satisfied when the request carries at least
/// one of its members. Multiple groups compose with AND (every
/// group must be satisfied).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RequiredOneOf {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<Vec<String>>>,
    /// Accepts both `headers` and `header` for parity with the
    /// allowlist itself.
    #[serde(default, alias = "header", skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<Vec<String>>>,
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

    /// Issue #75 — whether additive posture is on for this DSL.
    /// `Some(true)` → router does NOT filter undeclared fields out
    /// of `${incoming.*}`; the allowlist becomes documentation
    /// metadata only. Absent or `Some(false)` → traditional filter-
    /// and-continue.
    pub fn is_additive(&self) -> bool {
        self.additive.unwrap_or(false)
    }

    /// Issue #75 — validate mutually-exclusive posture flags.
    /// Returns `Err` if the declaration sets both `strict: true`
    /// and `additive: true` (contradictory — one rejects unknown
    /// keys, the other permits them). Called from the parser at
    /// load time so an operator gets a hard failure at boot instead
    /// of a silent one-wins-over-the-other at request time.
    pub fn validate_posture(&self) -> Result<(), String> {
        if self.is_strict() && self.is_additive() {
            return Err(
                "declaration.strict and declaration.additive are mutually exclusive \
                 (strict rejects unknown fields; additive permits them). Pick one."
                    .to_string(),
            );
        }
        // Issue #134 — a pass-through proxy route cannot also declare
        // a body allowlist. The body is opaque bytes forwarded to the
        // upstream; parsing it to apply an allowlist would defeat the
        // byte-identical contract (and the parse itself is where
        // compression bombs land). Same posture as
        // strict+additive: parse-time error, pick one.
        if let Some(proxy) = &self.proxy {
            if let Some(allowlist) = &self.allowlist {
                if allowlist.body.is_some() {
                    return Err(
                        "declaration.proxy and declaration.allowlist.body are mutually \
                         exclusive. A proxy route forwards the body byte-identically to \
                         the upstream; declaring a body allowlist would require parsing \
                         the body (defeats the contract and introduces a parser attack \
                         surface). Drop allowlist.body — upstream validates content shape."
                            .to_string(),
                    );
                }
            }
            if self.allowed_body.is_some() {
                return Err(
                    "declaration.proxy and declaration.allowed_body are mutually exclusive. \
                     Drop allowed_body; a proxy route does not parse the body."
                        .to_string(),
                );
            }
            if proxy.upstream.trim().is_empty() {
                return Err(
                    "declaration.proxy.upstream must be a non-empty absolute URL.".to_string(),
                );
            }
            if proxy.max_body_bytes == 0 {
                return Err("declaration.proxy.max_body_bytes must be > 0.".to_string());
            }
            for enc in &proxy.allowed_encodings {
                let normal = enc.trim().to_ascii_lowercase();
                if normal.is_empty() {
                    return Err(
                        "declaration.proxy.allowed_encodings contains an empty entry; drop it or replace with `identity`.".to_string(),
                    );
                }
                if !crate::dsl::KNOWN_CONTENT_ENCODINGS
                    .iter()
                    .any(|k| *k == normal)
                {
                    return Err(format!(
                        "declaration.proxy.allowed_encodings: unknown encoding '{}' (known: {}).",
                        enc,
                        crate::dsl::KNOWN_CONTENT_ENCODINGS.join(", ")
                    ));
                }
            }
        }
        Ok(())
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

//! OpenAPI 3.1 spec generator for the AS-IS DSL tree.
//!
//! Walks `LoadedProjects.http` and produces one `paths` entry per DSL
//! file. Extraction rules:
//!
//! - **Path** = `/<project>/<endpoint-path>` (Ruuter routes exact keys —
//!   no path templating yet; see task 018 in backlog).
//! - **Method** = the DSL's parent directory name lower-cased.
//! - **operationId** = `<method>_<project>_<slug>` — unique per route,
//!   safe as an identifier.
//! - **summary / description** = filename stem, plus `declaration.description`
//!   when the DSL includes a `declaration:` step (rare in current corpus).
//! - **Responses** = union of `status:` codes across all `return` steps in
//!   the DSL. Falls back to `200` when no explicit status is emitted or
//!   the status is a script expression we can't evaluate at generation
//!   time.
//! - **requestBody / parameters** = derived from `declaration.allowed_body`,
//!   `declaration.allowed_params`, `declaration.allowed_header` when
//!   present; otherwise omitted (open, best-effort inference).
//!
//! This is a documentation-only artefact — the generator is read-only.
//! Rebuilding the spec is cheap enough to do at boot; hot reload isn't
//! wired.

use crate::dsl::loader::{HttpDsls, LoadedProjects};
use crate::dsl::{DeclarationStep, Dsl, DslField};
use crate::steps::DslStep;
use serde_json::{json, Map, Value};

pub const OPENAPI_VERSION: &str = "3.1.0";

/// Methods OpenAPI's Path Item recognises. Everything else on the DSL
/// tree (e.g. `WS/` for WebSocket endpoints) is skipped — OpenAPI 3.1
/// does not model WebSocket contracts; that's AsyncAPI's job.
const OPENAPI_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD", "TRACE",
];

fn is_http_method(m: &str) -> bool {
    OPENAPI_HTTP_METHODS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(m))
}

/// Build a full OpenAPI document covering every loaded project.
pub fn build_spec(loaded: &LoadedProjects, service_version: &str) -> Value {
    build_spec_from_http(&loaded.http, service_version)
}

/// Same as `build_spec`, but takes only the HTTP tree. Useful for the
/// router which doesn't retain the triggers/guards views.
pub fn build_spec_from_http(http: &HttpDsls, service_version: &str) -> Value {
    let mut paths = Map::new();

    // Deterministic order — projects then dsl_key alpha — so a diff on the
    // spec file is meaningful across restarts.
    let mut project_names: Vec<&String> = http.keys().collect();
    project_names.sort();

    for project in project_names {
        let by_method = &http[project];
        let mut method_names: Vec<&String> = by_method.keys().collect();
        method_names.sort();
        for method in method_names {
            // Skip WebSocket and other non-HTTP method buckets — they
            // don't fit OpenAPI's Path Item shape.
            if !is_http_method(method) {
                continue;
            }
            let dsls = &by_method[method];
            let mut keys: Vec<&String> = dsls.keys().collect();
            keys.sort();
            for key in keys {
                let dsl = &dsls[key];
                // key looks like "GET/foo/bar"; strip the method prefix
                // and rebuild an OpenAPI path with the project prefix.
                let path_suffix = key.trim_start_matches(&format!("{}/", method));
                let http_path = format!("/{}/{}", project, path_suffix);
                let op = build_operation(project, method, key, dsl);

                let entry = paths
                    .entry(http_path)
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(map) = entry {
                    map.insert(method.to_lowercase(), op);
                }
            }
        }
    }

    json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": "Ruuter-on-Rust DSL API",
            "version": service_version,
            "description": "Auto-generated from the DSL tree on disk. \
                Endpoints reflect files present in DSL/<project>/<METHOD>/*.yml \
                at boot; the spec is regenerated on restart.",
            "license": {
                "name": "MIT",
                "identifier": "MIT"
            }
        },
        "servers": [
            {"url": "/", "description": "Same-origin (this Ruuter instance)"}
        ],
        "security": [],
        "paths": Value::Object(paths),
        "components": {
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": {"type": "string"}
                    },
                    "required": ["error"]
                }
            },
            "headers": {
                "X-Trace-Id": {
                    "description": "32-hex trace id extracted from the response's `traceparent`. Ops can grep for this across services.",
                    "schema": {"type": "string"}
                },
                "traceparent": {
                    "description": "W3C traceparent (`00-<trace_id>-<span_id>-<flags>`) either adopted from the request or generated fresh.",
                    "schema": {"type": "string"}
                }
            }
        }
    })
}

fn build_operation(project: &str, method: &str, dsl_key: &str, dsl: &Dsl) -> Value {
    let decl = dsl.declaration.as_ref();
    let statuses = collect_return_statuses(dsl);

    let mut op = Map::new();
    op.insert(
        "operationId".to_string(),
        Value::String(operation_id(project, method, dsl_key)),
    );
    op.insert(
        "summary".to_string(),
        Value::String(summary_from_key(dsl_key)),
    );
    // Description: prefer declared, else auto-derive so the rendered
    // spec doesn't show empty operation cards.
    let description = decl
        .and_then(|d| d.description.clone())
        .unwrap_or_else(|| {
            format!(
                "Auto-generated from DSL `{}` in project `{}`. Add a `declaration.description` to override.",
                dsl_key, project
            )
        });
    op.insert("description".to_string(), Value::String(description));
    op.insert("tags".to_string(), json!([project.to_string()]));

    if let Some(params) = build_parameters(decl) {
        op.insert("parameters".to_string(), params);
    }
    if let Some(body) = build_request_body(method, decl) {
        op.insert("requestBody".to_string(), body);
    }
    op.insert("responses".to_string(), build_responses(&statuses, decl));

    Value::Object(op)
}

fn operation_id(project: &str, method: &str, dsl_key: &str) -> String {
    let path_suffix = dsl_key.trim_start_matches(&format!("{}/", method));
    let slug: String = path_suffix
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{}_{}_{}", method.to_lowercase(), project, slug)
}

fn summary_from_key(dsl_key: &str) -> String {
    dsl_key.rsplit('/').next().unwrap_or(dsl_key).to_string()
}

fn collect_return_statuses(dsl: &Dsl) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for step in dsl.steps.values() {
        if let DslStep::Return(r) = step {
            match &r.status {
                Some(Value::Number(n)) => {
                    if let Some(u) = n.as_u64() {
                        if (100..=599).contains(&u) {
                            let code = u as u16;
                            if !out.contains(&code) {
                                out.push(code);
                            }
                        }
                    }
                }
                _ => {
                    // Missing / non-literal status → we can't statically
                    // resolve it, so fall through and let the default
                    // 200 response cover the shape.
                }
            }
        }
    }
    out
}

fn build_responses(statuses: &[u16], decl: Option<&DeclarationStep>) -> Value {
    let mut resp = Map::new();
    let mut effective: Vec<u16> = if statuses.is_empty() {
        vec![200]
    } else {
        statuses.to_vec()
    };
    // Ensure at least one 2xx (best-practice completeness). If the DSL
    // only sets 4xx literals, add a 200 for the happy path — the DSL
    // engine defaults to 200 when no explicit status is emitted.
    if !effective.iter().any(|c| (200..300).contains(c)) {
        effective.push(200);
    }
    // Framework-level baselines that every route can produce regardless
    // of what the DSL declares — advertise them so a partner-facing
    // spec doesn't lie about the surface:
    //   400: malformed JSON body on a `Content-Type: application/json` POST
    //   500: `Err(_) => INTERNAL_SERVER_ERROR` catch-all in handle_request
    if !effective.iter().any(|c| (400..500).contains(c)) {
        effective.push(400);
    }
    if !effective.contains(&500) {
        effective.push(500);
    }

    // Task 070 — structured returns:. Build the 2xx body schema once
    // from `declaration.returns` (typed properties + required array).
    // Applied to EVERY 2xx status code (a DSL that emits both 200 and
    // 202 for the same route returns the same shape).
    let typed_2xx_schema: Option<Value> = decl
        .and_then(|d| d.returns.as_ref())
        .filter(|fields| !fields.is_empty())
        .map(|fields| build_object_schema(fields));

    for code in effective {
        let key = code.to_string();
        let body_schema: Value = if code >= 400 {
            json!({"$ref": "#/components/schemas/Error"})
        } else if let Some(ref s) = typed_2xx_schema {
            s.clone()
        } else {
            // Ruuter returns application/json unconditionally today —
            // shape is DSL-defined, so we advertise "object" as the
            // broadest permitting shape without lying.
            json!({"type": "object", "additionalProperties": true})
        };
        resp.insert(
            key,
            json!({
                "description": status_reason(code),
                "headers": {
                    "traceparent": {"$ref": "#/components/headers/traceparent"},
                    "X-Trace-Id": {"$ref": "#/components/headers/X-Trace-Id"}
                },
                "content": {
                    "application/json": {
                        "schema": body_schema
                    }
                }
            }),
        );
    }
    Value::Object(resp)
}

/// Task 070 — map a `DslField` to an OpenAPI schema fragment. Only
/// the type / format / description / default / items keys emit; the
/// `field` name and `required` flag are consumed by callers (they
/// key the property map and the `required: [...]` array respectively).
fn field_schema(f: &DslField) -> Value {
    let mut schema = Map::new();
    let t = f.field_type.as_deref().unwrap_or("string");
    schema.insert("type".into(), Value::String(t.to_string()));
    if let Some(fmt) = &f.format {
        schema.insert("format".into(), Value::String(fmt.clone()));
    }
    if let Some(desc) = &f.description {
        schema.insert("description".into(), Value::String(desc.clone()));
    }
    if let Some(def) = &f.default {
        schema.insert("default".into(), def.clone());
    }
    if t == "array" {
        if let Some(items) = &f.items {
            schema.insert("items".into(), field_schema(items));
        } else {
            // Array with no `items:` declared — advertise loosely
            // rather than lying with a strict shape.
            schema.insert("items".into(), json!({}));
        }
    }
    Value::Object(schema)
}

/// Task 070 — build a `{ type: object, properties: ..., required: [...] }`
/// schema from a list of `DslField`s. Used for request bodies and
/// declared response shapes alike.
fn build_object_schema(fields: &[DslField]) -> Value {
    let mut props = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for f in fields {
        props.insert(f.field.clone(), field_schema(f));
        if f.required.unwrap_or(false) {
            required.push(Value::String(f.field.clone()));
        }
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String("object".into()));
    out.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    Value::Object(out)
}

fn build_parameters(decl: Option<&DeclarationStep>) -> Option<Value> {
    let decl = decl?;
    let mut params: Vec<Value> = Vec::new();

    // Task 070 — prefer the structured (per-field metadata) form
    // when the DSL declared `allowlist.params` / `allowlist.headers`;
    // fall back to the legacy flat list (name-only) for corpora that
    // haven't migrated.
    match decl.structured_params() {
        Some(fields) => {
            for f in fields {
                params.push(build_named_parameter(&f.field, "query", f));
            }
        }
        None => {
            if let Some(qs) = &decl.allowed_params {
                for name in qs {
                    params.push(json!({
                        "name": name,
                        "in": "query",
                        "required": false,
                        "schema": {"type": "string"}
                    }));
                }
            }
        }
    }

    match decl.structured_headers() {
        Some(fields) => {
            for f in fields {
                params.push(build_named_parameter(&f.field, "header", f));
            }
        }
        None => {
            if let Some(hs) = &decl.allowed_header {
                for name in hs {
                    params.push(json!({
                        "name": name,
                        "in": "header",
                        "required": false,
                        "schema": {"type": "string"}
                    }));
                }
            }
        }
    }

    if params.is_empty() {
        None
    } else {
        Some(Value::Array(params))
    }
}

fn build_named_parameter(name: &str, location: &str, f: &DslField) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), Value::String(name.to_string()));
    out.insert("in".into(), Value::String(location.to_string()));
    out.insert("required".into(), Value::Bool(f.required.unwrap_or(false)));
    if let Some(desc) = &f.description {
        out.insert("description".into(), Value::String(desc.clone()));
    }
    out.insert("schema".into(), field_schema(f));
    Value::Object(out)
}

fn build_request_body(method: &str, decl: Option<&DeclarationStep>) -> Option<Value> {
    // GET/DELETE routinely have no body; skip unless declaration is explicit.
    let body_bearing = matches!(method.to_uppercase().as_str(), "POST" | "PUT" | "PATCH");
    if !body_bearing {
        return None;
    }
    let decl = decl?;

    // Task 070 — prefer structured body (per-field metadata) when
    // declared; fall back to the legacy flat name list.
    let schema = match decl.structured_body() {
        Some(fields) if !fields.is_empty() => build_object_schema(fields),
        _ => {
            let allowed = decl.allowed_body.as_ref()?;
            if allowed.is_empty() {
                return None;
            }
            let mut props = Map::new();
            for name in allowed {
                props.insert(name.clone(), json!({"type": "string"}));
            }
            json!({
                "type": "object",
                "properties": Value::Object(props)
            })
        }
    };
    Some(json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": schema
            }
        }
    }))
}

fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Response",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::loader::LoadedProjects;
    use crate::dsl::Dsl;
    use crate::steps::{DslStep, ReturnStep};
    use indexmap::IndexMap;
    use std::collections::HashMap;

    fn dsl_with_return(status: Option<u16>) -> Dsl {
        let mut steps: IndexMap<String, DslStep> = IndexMap::new();
        steps.insert(
            "reply".into(),
            DslStep::Return(ReturnStep {
                return_value: json!("ok"),
                status: status.map(|s| json!(s)),
                headers: None,
                wrapper: None,
                next: None,
                base: Default::default(),
            }),
        );
        Dsl::new(steps)
    }

    fn loaded_with(project: &str, method: &str, key: &str, dsl: Dsl) -> LoadedProjects {
        let mut by_key: HashMap<String, Dsl> = HashMap::new();
        by_key.insert(format!("{}/{}", method, key), dsl);
        let mut by_method: HashMap<String, HashMap<String, Dsl>> = HashMap::new();
        by_method.insert(method.to_string(), by_key);
        let mut http = HashMap::new();
        http.insert(project.to_string(), by_method);
        LoadedProjects {
            http,
            triggers: HashMap::new(),
            guards: HashMap::new(),
        }
    }

    #[test]
    fn spec_emits_route_for_every_dsl() {
        let loaded = loaded_with("svc", "GET", "ping", dsl_with_return(Some(202)));
        let spec = build_spec(&loaded, "0.4.0");
        let paths = spec.get("paths").unwrap().as_object().unwrap();
        assert!(paths.contains_key("/svc/ping"));
        let op = &paths["/svc/ping"]["get"];
        assert_eq!(op["operationId"], "get_svc_ping");
        assert!(op["responses"].get("202").is_some());
    }

    #[test]
    fn spec_defaults_to_200_when_no_status_literal() {
        let loaded = loaded_with("svc", "POST", "orders", dsl_with_return(None));
        let spec = build_spec(&loaded, "0.4.0");
        let op = &spec["paths"]["/svc/orders"]["post"];
        assert!(op["responses"].get("200").is_some());
    }

    #[test]
    fn spec_error_status_uses_error_schema_ref() {
        let loaded = loaded_with("svc", "GET", "bad", dsl_with_return(Some(400)));
        let spec = build_spec(&loaded, "0.4.0");
        let op = &spec["paths"]["/svc/bad"]["get"];
        let schema = &op["responses"]["400"]["content"]["application/json"]["schema"];
        assert_eq!(schema["$ref"], "#/components/schemas/Error");
    }
}

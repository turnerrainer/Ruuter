//! Task 070 — declaration-parity regression tests.
//!
//! Covers the five coordinated behaviour changes:
//!
//! 1. Rich `DslField` — type / required / format / description /
//!    default / items parse alongside the bare `{field: X}` shape.
//! 2. OpenAPI enrichment — typed schemas, required arrays, response
//!    shape from `declaration.returns`.
//! 3. `strict: true` posture — unknown body / query / header keys
//!    return 400 instead of silently filtering.
//! 4. `warn_on_missing_declarations` — helper reports the correct
//!    count regardless of whether the WARN is emitted.
//! 5. Backwards-compat — bare `{field: X}` allowlist entries still
//!    parse, and the OpenAPI generator still produces a spec for
//!    them (no crashes on legacy corpora).

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::{warn_on_missing_declarations, DslLoader};
use ruuter_on_rust::dsl::parser::DslParser;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::openapi;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

fn write_dsl(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn build_router(dsl_root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("initial load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let state = StateStore::new();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http, guards, config, state, ws, engine,
    ))
}

async fn post_json(router: Arc<DslRouter>, path: &str, body: serde_json::Value) -> (u16, String) {
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_status(router: Arc<DslRouter>, path: &str) -> u16 {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    resp.status().as_u16()
}

// ============================================================================
// 1. Rich DslField parsing (backwards compatible)
// ============================================================================

#[test]
fn bare_field_shape_still_parses() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
declaration:
  allowlist:
    body:
      - field: userName
      - field: age
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse");
    let decl = dsl.declaration.expect("declaration present");
    let body = decl.structured_body().expect("body allowlist");
    assert_eq!(body.len(), 2);
    assert_eq!(body[0].field, "userName");
    // Bare form: type / required / format all None.
    assert_eq!(body[0].field_type, None);
    assert_eq!(body[0].required, None);
    assert_eq!(body[0].format, None);
}

#[test]
fn rich_field_shape_parses_typed_metadata() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
declaration:
  allowlist:
    body:
      - field: userName
        type: string
        required: true
        format: email
        description: "Login handle."
      - field: age
        type: integer
        default: 18
      - field: tags
        type: array
        items:
          field: __item__
          type: string
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse");
    let decl = dsl.declaration.expect("declaration present");
    let body = decl.structured_body().expect("body allowlist");
    assert_eq!(body[0].field_type.as_deref(), Some("string"));
    assert_eq!(body[0].required, Some(true));
    assert_eq!(body[0].format.as_deref(), Some("email"));
    assert_eq!(body[0].description.as_deref(), Some("Login handle."));
    assert_eq!(body[1].default, Some(serde_json::json!(18)));
    assert_eq!(body[2].field_type.as_deref(), Some("array"));
    let items = body[2].items.as_ref().expect("items present");
    assert_eq!(items.field_type.as_deref(), Some("string"));
}

// ============================================================================
// 2. OpenAPI enrichment
// ============================================================================

#[tokio::test]
async fn openapi_spec_carries_typed_body_schema() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declaration:
  description: "Create a user."
  allowlist:
    body:
      - field: userName
        type: string
        required: true
        format: email
      - field: age
        type: integer
        required: false
reply:
  return: "ok"
  status: 201
"#,
    );
    let router = build_router(tmp.path());
    let body = to_bytes(
        router
            .clone()
            .build_axum_router_from_arc()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/_/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body(),
        4 * 1024 * 1024,
    )
    .await
    .unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let op = &spec["paths"]["/svc/create"]["post"];
    let schema = &op["requestBody"]["content"]["application/json"]["schema"];
    // Typed body properties.
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["userName"]["type"], "string");
    assert_eq!(schema["properties"]["userName"]["format"], "email");
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    // Required array.
    assert_eq!(schema["required"], serde_json::json!(["userName"]));
}

#[tokio::test]
async fn openapi_spec_carries_typed_response_schema() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/user.yml",
        r#"
declaration:
  returns:
    - field: id
      type: integer
      required: true
    - field: email
      type: string
      format: email
reply:
  return:
    id: 1
    email: "a@b.c"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let body = to_bytes(
        router
            .build_axum_router_from_arc()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/_/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body(),
        4 * 1024 * 1024,
    )
    .await
    .unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let schema = &spec["paths"]["/svc/user"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["id"]["type"], "integer");
    assert_eq!(schema["properties"]["email"]["format"], "email");
    assert_eq!(schema["required"], serde_json::json!(["id"]));
}

#[tokio::test]
async fn openapi_spec_typed_query_params_carry_required_flag() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/search.yml",
        r#"
declaration:
  allowlist:
    params:
      - field: q
        type: string
        required: true
        description: "Search query."
      - field: limit
        type: integer
        required: false
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let body = to_bytes(
        router
            .build_axum_router_from_arc()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/_/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body(),
        4 * 1024 * 1024,
    )
    .await
    .unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let params = spec["paths"]["/svc/search"]["get"]["parameters"]
        .as_array()
        .expect("parameters emitted");
    let q = params.iter().find(|p| p["name"] == "q").expect("q param");
    assert_eq!(q["required"], true);
    assert_eq!(q["schema"]["type"], "string");
    assert_eq!(q["description"], "Search query.");
    let limit = params
        .iter()
        .find(|p| p["name"] == "limit")
        .expect("limit param");
    assert_eq!(limit["required"], false);
    assert_eq!(limit["schema"]["type"], "integer");
}

#[test]
fn openapi_spec_bare_declaration_still_produces_valid_spec() {
    use indexmap::IndexMap;
    use ruuter_on_rust::dsl::loader::LoadedProjects;
    use ruuter_on_rust::dsl::Dsl;
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
declaration:
  allowlist:
    body:
      - field: userName
      - field: age
reply:
  return: "ok"
  status: 201
"#,
        )
        .unwrap();
    let mut by_key: HashMap<String, Dsl> = HashMap::new();
    by_key.insert("POST/create".to_string(), dsl);
    let mut by_method: HashMap<String, HashMap<String, Dsl>> = HashMap::new();
    by_method.insert("POST".to_string(), by_key);
    let mut http = HashMap::new();
    http.insert("svc".to_string(), by_method);
    let loaded = LoadedProjects {
        http,
        triggers: HashMap::new(),
        guards: HashMap::new(),
    };
    let spec = openapi::build_spec(&loaded, "test");
    // Bare fields → still a properties map, but each field falls back
    // to `type: string` (the DslField default) and no required array.
    let schema = &spec["paths"]["/svc/create"]["post"]["requestBody"]["content"]
        ["application/json"]["schema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["userName"]["type"], "string");
    assert!(schema.get("required").is_none());
    // Silence indexmap unused warning.
    let _ = IndexMap::<String, ()>::new();
}

// ============================================================================
// 3. Strict-unknown-keys posture
// ============================================================================

#[tokio::test]
async fn strict_true_rejects_unknown_body_key_with_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declaration:
  strict: true
  allowlist:
    body:
      - field: userName
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, body) = post_json(
        router,
        "/svc/create",
        serde_json::json!({"userName": "alice", "surprise": "extra"}),
    )
    .await;
    assert_eq!(status, 400, "expected 400, got body: {body}");
    assert!(
        body.contains("Unexpected field") && body.contains("surprise"),
        "expected diagnostic naming the field, got: {body}"
    );
}

#[tokio::test]
async fn strict_false_default_silently_filters_unknown_key() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/create.yml",
        r#"
declaration:
  allowlist:
    body:
      - field: userName
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, _body) = post_json(
        router,
        "/svc/create",
        serde_json::json!({"userName": "alice", "surprise": "extra"}),
    )
    .await;
    // Non-strict posture: unknown key silently filtered, request succeeds.
    assert_eq!(status, 200);
}

#[tokio::test]
async fn strict_true_rejects_unknown_query_key_with_400() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/search.yml",
        r#"
declaration:
  strict: true
  allowlist:
    params:
      - field: q
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let status = get_status(router, "/svc/search?q=foo&sneaky=bar").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn strict_true_preserves_traceparent_header() {
    // traceparent is framework-injected. Strict-headers posture MUST
    // NOT 400 on it — otherwise every request under strict would fail.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/thing.yml",
        r#"
declaration:
  strict: true
  allowlist:
    headers:
      - field: x-tenant
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    // Client sends only the tenant header; framework injects traceparent
    // downstream. Should NOT 400 on the framework's own header.
    let req = Request::builder()
        .method("GET")
        .uri("/svc/thing")
        .header("x-tenant", "acme")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

// ============================================================================
// 4. Missing-declaration WARN helper
// ============================================================================

#[test]
fn missing_declaration_counter_counts_correctly() {
    use indexmap::IndexMap;
    use ruuter_on_rust::dsl::Dsl;
    use ruuter_on_rust::steps::{DslStep, ReturnStep};

    // Build a tiny tree: 3 HTTP DSLs, one with a declaration.
    let with_decl = {
        let parser = DslParser::new(HashMap::new());
        parser
            .parse_content(
                r#"
declaration:
  description: "declared"
reply:
  return: "ok"
  status: 200
"#,
            )
            .unwrap()
    };
    let without_decl = {
        let mut steps: IndexMap<String, DslStep> = IndexMap::new();
        steps.insert(
            "reply".into(),
            DslStep::Return(ReturnStep {
                return_value: serde_json::json!("ok"),
                status: Some(serde_json::json!(200)),
                headers: None,
                wrapper: None,
                next: None,
                base: Default::default(),
            }),
        );
        Dsl::new(steps)
    };
    let mut by_key: HashMap<String, Dsl> = HashMap::new();
    by_key.insert("GET/a".to_string(), with_decl);
    by_key.insert("GET/b".to_string(), without_decl.clone());
    by_key.insert("GET/c".to_string(), without_decl);
    let mut by_method: HashMap<String, HashMap<String, Dsl>> = HashMap::new();
    by_method.insert("GET".to_string(), by_key);
    let mut http = HashMap::new();
    http.insert("svc".to_string(), by_method);

    // Enabled = true: 2 WARN lines emitted, counter returns 2.
    let n_enabled = warn_on_missing_declarations(&http, true);
    assert_eq!(n_enabled, 2);
    // Enabled = false: same count, no WARN lines.
    let n_disabled = warn_on_missing_declarations(&http, false);
    assert_eq!(n_disabled, 2);
}

#[test]
fn missing_declaration_helper_skips_ws_bucket() {
    use indexmap::IndexMap;
    use ruuter_on_rust::dsl::Dsl;
    use ruuter_on_rust::steps::{DslStep, ReturnStep};

    // A DSL under WS/ (inbound WS frame handler) should NOT count as
    // a missing-declaration case — WS DSLs are not OpenAPI-shape.
    let ws_dsl = {
        let mut steps: IndexMap<String, DslStep> = IndexMap::new();
        steps.insert(
            "reply".into(),
            DslStep::Return(ReturnStep {
                return_value: serde_json::json!("ok"),
                status: Some(serde_json::json!(200)),
                headers: None,
                wrapper: None,
                next: None,
                base: Default::default(),
            }),
        );
        Dsl::new(steps)
    };
    let mut by_key: HashMap<String, Dsl> = HashMap::new();
    by_key.insert("WS/chat".to_string(), ws_dsl);
    let mut by_method: HashMap<String, HashMap<String, Dsl>> = HashMap::new();
    by_method.insert("WS".to_string(), by_key);
    let mut http = HashMap::new();
    http.insert("svc".to_string(), by_method);

    // Zero missing — the WS bucket is skipped entirely.
    assert_eq!(warn_on_missing_declarations(&http, true), 0);
}

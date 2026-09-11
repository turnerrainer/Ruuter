//! Issue #98 regression tests — Content-Type-driven response-body decode.
//!
//! Pre-fix Ruuter always attempted JSON parse on the upstream response
//! body regardless of `Content-Type`, and fell back to `Value::String`
//! on parse failure. That heuristic produced two subtle surprises:
//!
//! - A `text/plain` response whose body happened to be valid JSON
//!   (`123`, `null`, `"hello"`) reached the DSL as a JSON number /
//!   null / string, not as the raw text the wire declared.
//! - A UDS upstream returning non-JSON silently bound the DSL's
//!   `${result.response.body}` to `null` (the `.ok()` in
//!   `serde_json::from_slice(...).ok()`), discarding the payload.
//!
//! Post-fix Ruuter inspects `Content-Type` and parses JSON only when
//! the wire declares it (`application/json` or `application/*+json`,
//! with or without a media-type parameter). Everything else — including
//! missing `Content-Type` — arrives as a UTF-8 lossy string. A
//! `Content-Type: application/json` that lies (gateway 502 returning
//! HTML) logs a WARN and falls back to string so the DSL can still
//! forward / inspect.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::{content_type_is_json, decode_response_body, HttpClient};
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Unit tests — content-type matcher + decoder helper
// ---------------------------------------------------------------------------

#[test]
fn content_type_is_json_matches_bare_and_parameterised() {
    for ok in &[
        "application/json",
        "APPLICATION/JSON",
        "application/json; charset=utf-8",
        "application/json;charset=utf-8",
        "application/json; q=0.9",
        "  application/json  ",
    ] {
        assert!(content_type_is_json(ok), "expected JSON match for {ok:?}");
    }
}

#[test]
fn content_type_is_json_matches_structured_syntax_suffix() {
    for ok in &[
        "application/problem+json",
        "application/vnd.api+json",
        "application/hal+json",
        "application/ld+json; profile=\"https://schema.org/\"",
        "Application/Vnd.Custom+JSON",
    ] {
        assert!(content_type_is_json(ok), "expected +json match for {ok:?}");
    }
}

#[test]
fn content_type_is_json_rejects_non_json() {
    for miss in &[
        "",
        "text/plain",
        "text/plain; charset=utf-8",
        "text/xml",
        "text/json", // deliberately narrow — RFC-compliant JSON MIME is application/json
        "application/xml",
        "application/octet-stream",
        "image/png",
        "application/jsonp", // NOT +json, not application/json
        "json",
        "/json",
    ] {
        assert!(
            !content_type_is_json(miss),
            "expected non-JSON for {miss:?}"
        );
    }
}

#[test]
fn decode_response_body_parses_when_json_content_type_present() {
    let mut h = HashMap::new();
    h.insert(
        "content-type".to_string(),
        "application/json; charset=utf-8".to_string(),
    );
    let v = decode_response_body(br#"{"id":42,"name":"alice"}"#, &h).unwrap();
    assert_eq!(v, json!({"id": 42, "name": "alice"}));
}

#[test]
fn decode_response_body_parses_json_arrays() {
    let mut h = HashMap::new();
    h.insert("content-type".to_string(), "application/json".to_string());
    let v = decode_response_body(br#"[1,2,3]"#, &h).unwrap();
    assert_eq!(v, json!([1, 2, 3]));
}

#[test]
fn decode_response_body_parses_problem_json() {
    let mut h = HashMap::new();
    h.insert(
        "content-type".to_string(),
        "application/problem+json".to_string(),
    );
    let v = decode_response_body(
        br#"{"type":"about:blank","title":"Not Found","status":404}"#,
        &h,
    )
    .unwrap();
    assert_eq!(v["status"], 404);
    assert_eq!(v["title"], "Not Found");
}

#[test]
fn decode_response_body_returns_string_for_text_plain() {
    let mut h = HashMap::new();
    h.insert("content-type".to_string(), "text/plain".to_string());
    let v = decode_response_body(b"just some text here", &h).unwrap();
    assert_eq!(v, Value::String("just some text here".to_string()));
}

#[test]
fn decode_response_body_keeps_json_shaped_text_plain_as_string() {
    // Behaviour change: pre-#98 this would have been Value::Number(123).
    // Post-fix, the wire says text/plain, so the DSL gets "123".
    let mut h = HashMap::new();
    h.insert("content-type".to_string(), "text/plain".to_string());
    let v = decode_response_body(b"123", &h).unwrap();
    assert_eq!(v, Value::String("123".to_string()));

    let v_null = decode_response_body(b"null", &h).unwrap();
    assert_eq!(v_null, Value::String("null".to_string()));

    let v_bool = decode_response_body(b"true", &h).unwrap();
    assert_eq!(v_bool, Value::String("true".to_string()));

    let v_obj = decode_response_body(br#"{"a":1}"#, &h).unwrap();
    assert_eq!(v_obj, Value::String(r#"{"a":1}"#.to_string()));
}

#[test]
fn decode_response_body_returns_string_when_content_type_is_missing() {
    let h: HashMap<String, String> = HashMap::new();
    let v = decode_response_body(br#"{"a":1}"#, &h).unwrap();
    assert_eq!(v, Value::String(r#"{"a":1}"#.to_string()));
}

#[test]
fn decode_response_body_falls_back_to_string_on_bad_json() {
    // Gateway 502 pattern: Content-Type says JSON but the body is HTML.
    let mut h = HashMap::new();
    h.insert("content-type".to_string(), "application/json".to_string());
    let v = decode_response_body(b"<html>502 Bad Gateway</html>", &h).unwrap();
    assert_eq!(v, Value::String("<html>502 Bad Gateway</html>".to_string()));
}

#[test]
fn decode_response_body_empty_bytes_binds_empty_string_regardless_of_ct() {
    // Preserves the #63 contract: empty body → "" (not null), no
    // matter the Content-Type header.
    for ct in &["application/json", "text/plain", ""] {
        let mut h = HashMap::new();
        if !ct.is_empty() {
            h.insert("content-type".to_string(), ct.to_string());
        }
        let v = decode_response_body(b"", &h).unwrap();
        assert_eq!(v, Value::String(String::new()), "for CT {ct:?}");
    }
}

#[test]
fn decode_response_body_content_type_header_is_case_insensitive() {
    let mut h = HashMap::new();
    h.insert("Content-Type".to_string(), "application/json".to_string());
    let v = decode_response_body(br#"{"ok":true}"#, &h).unwrap();
    assert_eq!(v, json!({"ok": true}));
}

// ---------------------------------------------------------------------------
// End-to-end tests — DSL sees parsed JSON only when upstream says JSON
// ---------------------------------------------------------------------------

fn write_dsl(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn build_router(root: &Path) -> Arc<DslRouter> {
    let mut config = AppConfig::default();
    config.config_path = root.to_path_buf();
    config.internal_requests.block_private_networks = false;
    let loader = DslLoader::new(config.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    let http = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.http));
    let guards = Arc::new(arc_swap::ArcSwap::from_pointee(loaded.guards));
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&config))
        .with_ws_registry(ws.clone())
        .with_dsls_shared(http.clone());
    Arc::new(DslRouter::from_shared(
        http,
        guards,
        config,
        StateStore::new(),
        ws,
        engine,
    ))
}

async fn get(router: Arc<DslRouter>, path: &str) -> (u16, String) {
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// DSL that reflects what the upstream response body was decoded as.
/// The return payload is `{ "type": typeof body, "body": body }` — a
/// caller can assert on `type == "object"` / `"string"` / `"number"`
/// etc. and on the body value directly without wire-serialisation
/// tricks. `wrapper: false` skips the framework's default
/// `{"response": …}` envelope so the JSON parses cleanly.
async fn setup_reflect_route(tmp: &TempDir, upstream_url: &str) -> Arc<DslRouter> {
    write_dsl(
        tmp.path(),
        "svc/GET/proxy.yml",
        &format!(
            r#"
fetch:
  call: http.get
  args:
    url: "{upstream_url}"
  result: r
  next: reply
reply:
  return:
    type: "${{typeof r.response.body}}"
    body: "${{r.response.body}}"
  wrapper: false
  status: 200
"#
        ),
    );
    build_router(tmp.path())
}

fn reflect(response_wire_body: &str) -> Value {
    serde_json::from_str::<Value>(response_wire_body).unwrap_or_else(|e| {
        panic!("could not parse reflect envelope {response_wire_body:?}: {e}")
    })
}

#[tokio::test]
async fn e2e_application_json_2xx_is_parsed() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/j")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":42,"name":"alice"}"#)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/j", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (status, body) = get(router, "/svc/proxy").await;
    assert_eq!(status, 200);
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "object");
    assert_eq!(reflected["body"], json!({"id": 42, "name": "alice"}));
}

#[tokio::test]
async fn e2e_application_json_with_charset_is_parsed() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/j")
        .with_status(200)
        .with_header("content-type", "application/json; charset=utf-8")
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/j", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (status, body) = get(router, "/svc/proxy").await;
    assert_eq!(status, 200);
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "object");
    assert_eq!(reflected["body"], json!({"ok": true}));
}

#[tokio::test]
async fn e2e_json_array_is_parsed() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/list")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[1,2,3]")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/list", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (_status, body) = get(router, "/svc/proxy").await;
    let reflected = reflect(&body);
    // JS `typeof []` is `"object"` — arrays are objects in JS land.
    assert_eq!(reflected["type"], "object");
    assert_eq!(reflected["body"], json!([1, 2, 3]));
}

#[tokio::test]
async fn e2e_application_problem_json_is_parsed() {
    // RFC 7807 problem+json — proves `application/*+json` also hits
    // the parse path. `mockito` mock returns 200 so the http_client's
    // allow-list branch stays out of the picture; the wire status is
    // orthogonal to how the body is decoded.
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/fail")
        .with_status(200)
        .with_header("content-type", "application/problem+json")
        .with_body(r#"{"type":"about:blank","title":"Bad","status":400}"#)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/fail", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (_status, body) = get(router, "/svc/proxy").await;
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "object");
    assert_eq!(reflected["body"]["status"], 400);
    assert_eq!(reflected["body"]["title"], "Bad");
}

#[tokio::test]
async fn e2e_text_plain_valid_json_body_stays_string() {
    // Behaviour change vs pre-#98: an upstream that says
    // `Content-Type: text/plain` but sends a JSON-shaped body used to
    // be parsed as JSON. Post-fix the wire declares text/plain, so
    // the DSL gets the raw string and the author can `JSON.parse`
    // explicitly if they want.
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/plain-json")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/plain-json", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (_status, body) = get(router, "/svc/proxy").await;
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "string");
    assert_eq!(reflected["body"], json!(r#"{"ok":true}"#));
}

#[tokio::test]
async fn e2e_invalid_json_under_application_json_falls_back_to_string() {
    // Gateway-502 pattern: upstream declares JSON, returns HTML.
    // Ruuter logs a WARN (see decode_response_body) and hands the DSL
    // a raw string so the workflow can still emit a semantic 502.
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/gateway-502")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("<html>502 Bad Gateway</html>")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/gateway-502", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (_status, body) = get(router, "/svc/proxy").await;
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "string");
    assert_eq!(reflected["body"], json!("<html>502 Bad Gateway</html>"));
}

#[tokio::test]
async fn e2e_text_xml_stays_as_string() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("GET", "/xml")
        .with_status(200)
        .with_header("content-type", "text/xml")
        .with_body("<root><item>hello</item></root>")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/xml", server.url());
    let router = setup_reflect_route(&tmp, &url).await;
    let (_status, body) = get(router, "/svc/proxy").await;
    let reflected = reflect(&body);
    assert_eq!(reflected["type"], "string");
    assert_eq!(reflected["body"], json!("<root><item>hello</item></root>"));
}

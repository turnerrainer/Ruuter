//! Audit finding 11 regression tests — inbound + outbound content-type
//! dispatch (Java-parity).

use axum::body::{to_bytes, Body};
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
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

async fn post(router: Arc<DslRouter>, path: &str, ct: &str, body: Vec<u8>) -> (u16, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", ct)
        .body(Body::from(body))
        .unwrap();
    let resp = router.build_axum_router_from_arc().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ── Inbound ──────────────────────────────────────────────────────

#[tokio::test]
async fn inbound_form_urlencoded_body_is_parsed_into_keys() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/form.yml",
        r#"
reply:
  return:
    userId: "${incoming.body.userId}"
    tag: "${incoming.body.tag}"
  status: 200
"#,
    );
    let (status, body) = post(
        build_router(tmp.path()),
        "/svc/form",
        "application/x-www-form-urlencoded",
        b"userId=42&tag=alpha".to_vec(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.contains("\"userId\":\"42\""), "urlencoded parsed: {body}");
    assert!(body.contains("\"tag\":\"alpha\""), "urlencoded parsed: {body}");
}

#[tokio::test]
async fn inbound_text_plain_body_wrapped_under_subtype_key() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/text.yml",
        r#"
reply:
  return: "${incoming.body.plain}"
  status: 200
"#,
    );
    let (status, body) = post(
        build_router(tmp.path()),
        "/svc/text",
        "text/plain",
        b"hello".to_vec(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, "\"hello\"");
}

#[tokio::test]
async fn inbound_multipart_form_data_parses_file_parts() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/POST/upload.yml",
        r#"
reply:
  return: "${incoming.body['note.txt']}"
  status: 200
"#,
    );

    let boundary = "----RuuterAuditFixBoundary";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\n\r\nhello upload\r\n--{b}--\r\n",
        b = boundary
    );
    let (status, resp_body) = post(
        build_router(tmp.path()),
        "/svc/upload",
        &format!("multipart/form-data; boundary={}", boundary),
        body.into_bytes(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(resp_body, "\"hello upload\"");
}

// ── Outbound content_type on HttpStep ────────────────────────────

#[tokio::test]
async fn outbound_content_type_plaintext_sends_text_plain() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("POST", "/echo")
        .match_header("content-type", mockito::Matcher::Regex("^text/plain".into()))
        .match_body(mockito::Matcher::Exact("greetings".to_string()))
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/echo", server.url());
    write_dsl(
        tmp.path(),
        "svc/GET/send.yml",
        &format!(
            r#"
call:
  call: http.post
  args:
    url: "{url}"
    body:
      text: "greetings"
    content_type: plaintext
  result: r
  next: reply
reply:
  return: "${{r.response.body.ok}}"
  status: 200
"#,
            url = url
        ),
    );
    let router = build_router(tmp.path());
    let req = Request::builder()
        .method("GET")
        .uri("/svc/send")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), "true");
    m.assert_async().await;
}

#[tokio::test]
async fn outbound_content_type_formdata_sends_urlencoded() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("POST", "/submit")
        .match_header(
            "content-type",
            mockito::Matcher::Regex("application/x-www-form-urlencoded".into()),
        )
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("userId=42".into()),
            mockito::Matcher::Regex("amount=100".into()),
        ]))
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .with_header("content-type", "application/json")
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let url = format!("{}/submit", server.url());
    write_dsl(
        tmp.path(),
        "svc/GET/submit.yml",
        &format!(
            r#"
call:
  call: http.post
  args:
    url: "{url}"
    body:
      userId: 42
      amount: 100
    content_type: formdata
  result: r
  next: reply
reply:
  return: "${{r.response.body.ok}}"
  status: 200
"#,
            url = url
        ),
    );
    let router = build_router(tmp.path());
    let req = Request::builder()
        .method("GET")
        .uri("/svc/submit")
        .body(Body::empty())
        .unwrap();
    let resp = router
        .build_axum_router_from_arc()
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    m.assert_async().await;
}

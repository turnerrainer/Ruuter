//! Issues #28 + #29 regression tests — the caller-facing error
//! response body must name the step (issue #28) AND include the
//! source() chain (issue #29). Before these fixes:
//!
//!   #28: `{"error":"Script evaluation error: TypeError: ..."}`
//!        — no step name, no project, no DSL context.
//!   #29: `{"error":"HTTP error: error sending request for url (...)"}`
//!        — no underlying cause (DNS failure, connection refused,
//!        TLS handshake, etc.), all hidden inside .source().

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

// ============================================================================
// #28 — JS expression failure surfaces step + project in the response
// ============================================================================

#[tokio::test]
async fn js_expression_failure_names_the_step_and_project() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "myproject/GET/oops.yml",
        // TypeError: cannot read properties of null. Historically this
        // test used an undeclared identifier (`undefined_var.some_field`),
        // but #57 made undeclared identifiers evaluate to `undefined`
        // rather than throw ReferenceError — so triggering a diagnostic
        // now needs a real type-error (deref on a declared-but-null
        // value). Same diagnostic-surface contract from #28 still holds.
        r#"
respond:
  return: "${(null).some_field}"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, body) = get(router, "/myproject/oops").await;

    assert_eq!(
        status, 500,
        "JS error surfaces as 500 (this is the pre-existing framework contract)"
    );
    // The response body MUST identify WHICH step + WHICH project failed.
    assert!(
        body.contains("respond"),
        "response must name the failing step, got: {body}"
    );
    assert!(
        body.contains("myproject"),
        "response must name the project, got: {body}"
    );
    // The response body MUST include the underlying script error.
    assert!(
        body.to_lowercase().contains("script") || body.to_lowercase().contains("evaluation"),
        "response must include the underlying script/evaluation diagnostic, got: {body}"
    );
    // And the "caused by" chain must be present (StepContext + Script eval hops).
    assert!(
        body.contains("caused by"),
        "response must render the source() chain via error_chain(), got: {body}"
    );
}

#[tokio::test]
async fn js_expression_failure_includes_step_type() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/broken.yml",
        // TypeError trigger — see the sibling test above for why an
        // undeclared identifier no longer works post-#57.
        r#"
compute:
  assign:
    x: "${(null).nested}"
respond:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (_status, body) = get(router, "/svc/broken").await;
    // Step type helps operator distinguish `assign` from `http` from
    // `return` etc. — very useful when the same DSL has many steps.
    assert!(
        body.contains("(assign)"),
        "response must include the step type, got: {body}"
    );
    assert!(
        body.contains("compute"),
        "response must name the failing step, got: {body}"
    );
}

// ============================================================================
// #29 — HTTP request failure surfaces the reqwest/hyper cause chain
// ============================================================================

#[tokio::test]
async fn http_step_failure_surfaces_underlying_cause() {
    let tmp = TempDir::new().unwrap();
    // Deliberately unreachable target: port 1 is IANA-reserved
    // "tcpmux" and effectively never in use on a dev host, so the
    // connect attempt fails with an OS-level error. reqwest wraps
    // that as .source() inside its Display-hidden layers.
    write_dsl(
        tmp.path(),
        "svc/GET/upstream.yml",
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:1/never"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/upstream").await;

    assert_eq!(status, 500);
    // #28: response identifies the failing step + project.
    assert!(body.contains("fetch"), "step name required, got: {body}");
    assert!(body.contains("svc"), "project name required, got: {body}");
    // #29 core assertion: response includes the ACTUAL cause
    // (connect / DNS / IO error), not just the generic
    // "error sending request for url" that reqwest's Display emits.
    // Different platforms surface connect failures with slightly
    // different messages (ECONNREFUSED / "Connection refused" /
    // "actively refused" on Windows, etc.) — match on any of the
    // common signatures.
    let lower = body.to_lowercase();
    let has_cause = [
        "refused",
        "unreachable",
        "timed out",
        "no route",
        "os error",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    assert!(
        has_cause,
        "response must include the underlying connect failure cause, got: {body}"
    );
    // And the chain must be walked (multi-hop).
    assert!(
        body.contains("caused by"),
        "response must render the source() chain, got: {body}"
    );
}

// ============================================================================
// Baseline — non-error responses unaffected by the enrichment.
// ============================================================================

#[tokio::test]
async fn healthy_response_untouched_by_error_enrichment() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "svc/GET/ok.yml",
        r#"
respond:
  return: "hello"
  status: 200
"#,
    );
    let router = build_router(tmp.path());
    let (status, body) = get(router, "/svc/ok").await;
    assert_eq!(status, 200);
    // No cause chain nonsense in a happy-path response.
    assert!(
        !body.contains("caused by"),
        "healthy body must be pristine, got: {body}"
    );
    // The default wrapper still applies.
    assert!(
        body.contains("response") && body.contains("hello"),
        "wrapped response shape, got: {body}"
    );
}

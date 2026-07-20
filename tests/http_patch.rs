//! Acceptance test for the `http.patch` DSL step (task 023).
//!
//! Every case proves the outgoing HTTP method is literally PATCH and
//! that the response shape reaches the caller intact.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::context::ExecutionContext;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::steps::{HttpArgs, HttpStep, DslStep};
use ruuter_on_rust::dsl::Dsl;
use indexmap::IndexMap;
use serde_json::json;
use std::collections::HashMap;

async fn run_patch(url: String, timeout_ms: Option<u64>) -> Result<serde_json::Value, String> {
    let mut steps: IndexMap<String, DslStep> = IndexMap::new();
    steps.insert(
        "call".into(),
        DslStep::Http(HttpStep {
            call: "http.patch".into(),
            args: HttpArgs {
                url,
                body: Some(json!({"note": "ratchet"})),
                query: None,
                headers: None,
                content_type: None,
            },
            result: Some("out".into()),
            next: Some("end".into()),
            error: None,
            timeout: timeout_ms,
        }),
    );
    let dsl = Dsl::new(steps);

    let mut cfg = AppConfig::default();
    // The mockito fixture binds on 127.0.0.1; disable the N4 default
    // private-network block so the acceptance test can hit it.
    cfg.internal_requests.block_private_networks = false;
    let engine = StepEngine::new(HttpClient::new(&cfg));
    let ctx = ExecutionContext::new(HashMap::new(), HashMap::new(), HashMap::new(), "test".into());
    engine.run(&dsl, &ctx).await.map_err(|e| e.to_string())?;
    ctx.get_variable("out").ok_or_else(|| "result binding missing".to_string())
}

#[tokio::test]
async fn http_patch_200_ok_preserves_body() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PATCH", "/orders/o-1")
        .match_body(mockito::Matcher::JsonString(r#"{"note":"ratchet"}"#.to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"o-1","stop_price":123.45}"#)
        .create_async()
        .await;

    let out = run_patch(format!("{}/orders/o-1", server.url()), None)
        .await
        .expect("step");
    assert_eq!(out["response"]["status"], 200);
    assert_eq!(out["response"]["body"]["id"], "o-1");
    assert_eq!(out["response"]["body"]["stop_price"], 123.45);
    m.assert_async().await;
}

#[tokio::test]
async fn http_patch_4xx_response_reaches_caller_intact() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PATCH", "/orders/o-2")
        .with_status(409)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":"already_filled"}"#)
        .create_async()
        .await;

    let out = run_patch(format!("{}/orders/o-2", server.url()), None)
        .await
        .expect("step");
    assert_eq!(out["response"]["status"], 409);
    assert_eq!(out["response"]["body"]["error"], "already_filled");
    m.assert_async().await;
}

#[tokio::test]
async fn http_patch_5xx_bubbles_status() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PATCH", "/orders/o-3")
        .with_status(503)
        .with_body("service unavailable")
        .create_async()
        .await;

    let out = run_patch(format!("{}/orders/o-3", server.url()), None)
        .await
        .expect("step");
    assert_eq!(out["response"]["status"], 503);
    m.assert_async().await;
}

#[tokio::test]
async fn http_patch_timeout_errors_out() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("PATCH", "/orders/slow")
        .with_status(200)
        .with_chunked_body(|w| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            w.write_all(b"{}")
        })
        .create_async()
        .await;

    let result = run_patch(format!("{}/orders/slow", server.url()), Some(50)).await;
    assert!(
        result.is_err(),
        "step must fail when upstream exceeds the configured timeout, got {:?}",
        result
    );
}

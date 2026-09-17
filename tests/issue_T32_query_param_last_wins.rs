//! h2ck.me v1 T-32 — pin the `incoming.params` last-wins semantic
//! for duplicate query parameter keys.
//!
//! `?x=a&x=b&x=c` collapses to `{"x": "c"}` today because
//! `router::handle_request` parses the query string with
//! `url::form_urlencoded::parse` and collects into a
//! `HashMap<String, String>` — `FromIterator` for `HashMap`
//! processes pairs in iteration order and each insert replaces
//! the previous value, so the LAST occurrence wins.
//!
//! The docs section added under `book/src/dsl/context.md`
//! documents this. This test is the safety net: if a future
//! refactor swaps the container or the iteration order, DSL
//! authors who read the docs would silently see different
//! behaviour without a compile-time signal. The tests here fail
//! loudly the moment the semantic changes.
//!
//! WebSocket handshake query parameters go through the same
//! parser and get the same treatment; a WS-specific regression
//! test is deliberately not included here — the HTTP parser is
//! the shared source of truth and pinning it is sufficient.
//!
//! Tests written to try to BREAK the fix:
//! - `?x=a&x=b` → `${incoming.params.x}` is `"b"` (last wins).
//! - `?x=a&x=b&x=c` → `${incoming.params.x}` is `"c"`.
//! - `?first=1&second=2` → both keys present, values as sent
//!   (no cross-key mangling).
//! - `?x=a` (single value) → `"a"` (baseline).

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

fn uuid() -> String {
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build_router() -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-T32-{}", uuid()));
    let dsl_path = tmp.join("svc/GET/echo.yml");
    std::fs::create_dir_all(dsl_path.parent().unwrap()).unwrap();
    std::fs::write(
        &dsl_path,
        r#"
reply:
  return:
    x: "${incoming.params.x ?? 'MISSING'}"
    first: "${incoming.params.first ?? 'MISSING'}"
    second: "${incoming.params.second ?? 'MISSING'}"
  next: end
"#,
    )
    .unwrap();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(
        HttpClient::new(&cfg),
        ruuter_on_rust::steps::engine::empty_shared_guards(),
        cfg.guards.mode,
    )
    .with_ws_registry(ws.clone())
    .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

async fn serve(router: DslRouter) -> u16 {
    let app = router.build_axum_router();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    port
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
}

async fn fetch_query(url: &str) -> serde_json::Value {
    let router = build_router();
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/{}", port, url))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, 200, "echo endpoint must return 200; body={}", text);
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    body["response"].clone()
}

#[tokio::test]
async fn duplicate_key_last_wins_two_values() {
    let query = fetch_query("svc/echo?x=a&x=b").await;
    assert_eq!(
        query["x"], "b",
        "duplicate query key must resolve to the LAST occurrence \
         (last-wins semantic pinned by T-32); got {query}"
    );
}

#[tokio::test]
async fn duplicate_key_last_wins_three_values() {
    let query = fetch_query("svc/echo?x=a&x=b&x=c").await;
    assert_eq!(
        query["x"], "c",
        "three-value duplicate must resolve to the last (c); got {query}"
    );
}

#[tokio::test]
async fn unique_keys_survive_unchanged() {
    let query = fetch_query("svc/echo?first=1&second=2").await;
    assert_eq!(query["first"], "1");
    assert_eq!(query["second"], "2");
    // `x` was not sent, so the DSL renders the `?? 'MISSING'`
    // fallback — proves the fallback path is exercised and the
    // "missing key" observation is deterministic.
    assert_eq!(query["x"], "MISSING");
}

#[tokio::test]
async fn single_value_baseline() {
    let query = fetch_query("svc/echo?x=a").await;
    assert_eq!(query["x"], "a");
    assert_eq!(query["first"], "MISSING");
    assert_eq!(query["second"], "MISSING");
}

/// Adjacent-duplicate case: value `a` appears twice under the same
/// key without a differing sibling. Last-wins still applies, but
/// with an identical value the observable outcome is the same as
/// "a single value" — this guards against a refactor that would
/// dedupe identical adjacent duplicates as a "smart" optimisation
/// (which would change observable behaviour if the two values
/// were represented differently, e.g. one URL-encoded).
#[tokio::test]
async fn duplicate_key_same_value_still_admits() {
    let query = fetch_query("svc/echo?x=a&x=a").await;
    assert_eq!(query["x"], "a");
}

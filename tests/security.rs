//! Task 031 — end-to-end tests for the 0.4.0 security surfaces.
//!
//! Every test spins the real `DslRouter` on a random localhost port
//! via `axum::serve` and drives it with `reqwest`, so a regression
//! in the wiring order inside `router::handle_request` shows up
//! here (unit tests on the modules alone can't catch that).

// Test-fixture AppConfig assembly. See tests/trigger_dispatch.rs.
#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{
    AppConfig, CorsConfig, CsrfConfig, IncomingRequestsConfig, InternalRequestsConfig,
};
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build_router(cfg: AppConfig, files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-sec-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = cfg;
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(HttpClient::new(&cfg))
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
    // Yield so the server is actually listening by the time reqwest fires.
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

// ── CSRF ────────────────────────────────────────────────────────────

#[tokio::test]
async fn csrf_rejects_state_change_from_disallowed_origin() {
    let mut cfg = AppConfig::default();
    cfg.csrf = CsrfConfig {
        allowed_origins: vec!["https://allowed.example.com".to_string()],
        enforce_on_methods: vec!["POST".into()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/POST/ping.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let r = client()
        .post(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("origin", "https://evil.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn csrf_bypassed_when_allowed_origins_empty() {
    let cfg = AppConfig::default(); // allowed_origins is empty by default
    let router = build_router(
        cfg,
        &[(
            "svc/POST/ping.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let r = client()
        .post(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("origin", "https://any.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

// ── Method allow-list ──────────────────────────────────────────────

#[tokio::test]
async fn method_not_in_allow_list_returns_405() {
    let mut cfg = AppConfig::default();
    cfg.incoming_requests = IncomingRequestsConfig {
        allowed_method_types: vec!["GET".into()],
        headers: HashMap::new(),
    };
    let router = build_router(
        cfg,
        &[(
            "svc/POST/ping.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let r = client()
        .post(format!("http://127.0.0.1:{}/svc/ping", port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 405);
}

// ── Idempotency-Key ────────────────────────────────────────────────
//
// Framework-level Idempotency-Key handling was removed in v1.0.0
// (h2ck.me findings S1 + S5). This test pins the new contract:
// the framework does NOT cache or replay; two identical requests
// with the same `Idempotency-Key` both execute the DSL. DSL authors
// implement idempotency via `state.get`/`state.set` with their own
// identity + body-hash keys — see book/src/dsl/idempotency-pattern.md.

#[tokio::test]
async fn idempotency_key_header_is_not_handled_by_framework() {
    let cfg = AppConfig::default();
    // A counter DSL — if the framework were caching, the second
    // call would replay `value: 1`. Now it re-runs and returns 2.
    let router = build_router(
        cfg,
        &[(
            "svc/POST/inc.yml",
            r#"
bump:
  state:
    get: { key: "n", into: prev }
  next: shape
shape:
  assign: { current: "${(prev ?? 0) + 1}" }
  next: write
write:
  state: { set: { key: "n", value: "${current}" } }
  next: reply
reply:
  return: { value: "${current}" }
  status: 200
  next: end
"#,
        )],
    );
    let port = serve(router).await;

    let c = client();
    let r1: serde_json::Value = c
        .post(format!("http://127.0.0.1:{}/svc/inc", port))
        .header("idempotency-key", "abc-123")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r1["value"], 1);

    let resp2 = c
        .post(format!("http://127.0.0.1:{}/svc/inc", port))
        .header("idempotency-key", "abc-123")
        .send()
        .await
        .unwrap();
    // No `Idempotency-Replayed` header — the framework never
    // emits it any more.
    assert!(
        resp2.headers().get("idempotency-replayed").is_none(),
        "framework must not emit Idempotency-Replayed after v1.0 removal"
    );
    let r2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(
        r2["value"], 2,
        "second call must re-run the DSL (framework no longer caches by Idempotency-Key)"
    );
}

// ── Traceparent ────────────────────────────────────────────────────

#[tokio::test]
async fn traceparent_adopted_from_request_and_echoed() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let incoming = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("traceparent", incoming)
        .send()
        .await
        .unwrap();
    let tp = resp
        .headers()
        .get("traceparent")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let xtid = resp
        .headers()
        .get("x-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(tp, incoming, "traceparent must be echoed verbatim");
    assert_eq!(
        xtid, "4bf92f3577b34da6a3ce929d0e0e4736",
        "x-trace-id must be the 32-hex trace id"
    );
}

// ── SSRF ────────────────────────────────────────────────────────────

#[tokio::test]
async fn ssrf_outbound_disabled_blocks_http_step() {
    // internal_requests.disabled=true → http.get fails without ever
    // reaching the network.
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: true,
        allowed_ips: vec![],
        allowed_urls: vec![],
        block_private_networks: false,
    };
    let router = build_router(
        cfg,
        &[(
            "svc/GET/call.yml",
            r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:1/should-be-blocked"
  result: r
  next: reply
reply:
  return: { got: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/call", port))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        500,
        "outbound-disabled must surface as 500 to the caller"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("outbound HTTP is disabled"),
        "message: {}",
        err
    );
}

// ── Malformed JSON ──────────────────────────────────────────────────

#[tokio::test]
async fn malformed_json_body_returns_400() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let r = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

// ── CORS ────────────────────────────────────────────────────────────

#[tokio::test]
async fn cors_adds_allow_origin_header_when_configured() {
    let mut cfg = AppConfig::default();
    cfg.cors = CorsConfig {
        allowed_origins: vec!["https://ui.example.com".to_string()],
        allow_credentials: false,
    };
    let router = build_router(
        cfg,
        &[(
            "svc/GET/ping.yml",
            "respond: { return: { ok: true }, next: end }\n",
        )],
    );
    let port = serve(router).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("origin", "https://ui.example.com")
        .send()
        .await
        .unwrap();
    let acao = resp.headers().get("access-control-allow-origin").cloned();
    assert_eq!(
        acao.map(|v| v.to_str().unwrap().to_string()),
        Some("https://ui.example.com".to_string())
    );
}

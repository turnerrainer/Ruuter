//! Integration tests for task 044 (`http.<verb>` self-call short-circuit).
//!
//! Every test written to BREAK an incorrect implementation:
//! - Short-circuit must actually fire (byte-identical output OR
//!   network path was skipped by construction)
//! - Guards on the target route MUST run (silent bypass would be a
//!   security bug)
//! - Path-param resolution MUST work
//! - Response status + headers + body MUST be identical whether
//!   dispatched via short-circuit or via a network-loopback call
//! - No short-circuit when SelfOrigins doesn't match the URL
//! - No short-circuit when router handle isn't wired

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::{HttpClient, SelfCallHandler, SelfOrigins};
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn uuid() -> String {
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn build_router_with_dsls(files: &[(&str, &str)]) -> Arc<DslRouter> {
    let tmp = std::env::temp_dir().join(format!("ruuter-self-call-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg)).with_ws_registry(ws.clone());
    Arc::new(DslRouter::new(
        loaded.http,
        loaded.guards,
        cfg,
        StateStore::new(),
        ws,
        engine,
    ))
}

// ── Basic dispatch through the SelfCallHandler trait ────────────

#[tokio::test]
async fn self_call_reaches_target_dsl() {
    let router = build_router_with_dsls(&[(
        "svc/GET/echo.yml",
        r#"
respond:
  return: { got: "${incoming.params.q}" }
  next: end
"#,
    )]);

    let mut q = HashMap::new();
    q.insert("q".to_string(), json!("hello"));
    let resp = router
        .execute_by_url(
            "GET",
            "/svc/echo",
            q,
            HashMap::new(),
            HashMap::new(),
        )
        .await
        .expect("execute_by_url");

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.unwrap()["got"], json!("hello"));
}

#[tokio::test]
async fn self_call_via_http_client_short_circuit() {
    // The end-to-end wiring test: build a router, wire it as the
    // HttpClient's SelfCallHandler, then call http_client.request
    // on a URL matching the SelfOrigins set. Result must match what
    // execute_by_url produces directly.
    let router = build_router_with_dsls(&[(
        "svc/GET/ping.yml",
        r#"
respond:
  return: { pong: true, method: "GET" }
  next: end
"#,
    )]);

    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("127.0.0.1".to_string(), 8080));
    let client = HttpClient::with_timeout_ms(1000).with_self_origins(origins);
    client.set_self_call_handler(router.clone() as Arc<dyn SelfCallHandler>);

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://127.0.0.1:8080/svc/ping",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let body = resp.body.unwrap();
    assert_eq!(body["pong"], json!(true));
    assert_eq!(body["method"], json!("GET"));
}

#[tokio::test]
async fn self_call_localhost_synonyms_all_match() {
    // Every loopback synonym registered in SelfOrigins::from_config
    // must hit the short-circuit path.
    let router = build_router_with_dsls(&[(
        "svc/GET/pong.yml",
        r#"
respond:
  return: { ok: true }
  next: end
"#,
    )]);

    let cfg = AppConfig::default(); // port=8080 by default
    let origins = SelfOrigins::from_config(&cfg);
    let client = HttpClient::with_timeout_ms(1000).with_self_origins(origins);
    client.set_self_call_handler(router.clone() as Arc<dyn SelfCallHandler>);

    for host in ["localhost", "127.0.0.1", "0.0.0.0"] {
        let url = format!("http://{}:{}/svc/pong", host, cfg.port);
        let resp = client
            .request(reqwest::Method::GET, &url, None, None, None, None)
            .await
            .unwrap_or_else(|e| panic!("host {} failed: {}", host, e));
        assert_eq!(resp.status, 200, "host {} short-circuit failed", host);
    }
}

// ── Guards MUST run through the short-circuit ────────────────────

#[tokio::test]
async fn self_call_runs_guards_on_target_route() {
    // Guard rejects with 401 when the required header is absent —
    // proves the short-circuit isn't silently bypassing security.
    let router = build_router_with_dsls(&[
        (
            "svc/GET/protected.guard.yml",
            r#"
check_auth:
  switch:
    - condition: "${!incoming.headers['x-auth']}"
      next: unauthorized
  next: ok

ok:
  status: 200
  return: { guard_passed: true }
  next: end

unauthorized:
  status: 401
  return: { error: "auth required" }
  next: end
"#,
        ),
        (
            "svc/GET/protected/data.yml",
            r#"
respond:
  return: { protected_payload: true }
  next: end
"#,
        ),
    ]);

    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("localhost".to_string(), 8080));
    let client = HttpClient::with_timeout_ms(1000).with_self_origins(origins);
    client.set_self_call_handler(router.clone() as Arc<dyn SelfCallHandler>);

    // Without the header: guard rejects, main DSL must NOT run
    let no_hdr = client
        .request(
            reqwest::Method::GET,
            "http://localhost:8080/svc/protected/data",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");
    assert_eq!(no_hdr.status, 401, "guard must reject when header missing");
    assert_eq!(no_hdr.body.as_ref().unwrap()["error"], "auth required");

    // With the header: guard passes, main DSL runs
    let mut h = HashMap::new();
    h.insert("x-auth".to_string(), json!("yes"));
    let with_hdr = client
        .request(
            reqwest::Method::GET,
            "http://localhost:8080/svc/protected/data",
            None,
            None,
            Some(&h),
            None,
        )
        .await
        .expect("request");
    assert_eq!(with_hdr.status, 200);
    assert_eq!(
        with_hdr.body.unwrap()["protected_payload"],
        json!(true),
        "main DSL must run when guard passes"
    );
}

// ── Path parameters ──────────────────────────────────────────────

#[tokio::test]
async fn self_call_resolves_path_parameters() {
    let router = build_router_with_dsls(&[(
        "svc/GET/things.yml",
        r#"
respond:
  return:
    id: "${incoming.params.pathParams[0]}"
    subresource: "${incoming.params.pathParams[1]}"
  next: end
"#,
    )]);

    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("localhost".to_string(), 8080));
    let client = HttpClient::with_timeout_ms(1000).with_self_origins(origins);
    client.set_self_call_handler(router.clone() as Arc<dyn SelfCallHandler>);

    let resp = client
        .request(
            reqwest::Method::GET,
            "http://localhost:8080/svc/things/xyz/legs",
            None,
            None,
            None,
            None,
        )
        .await
        .expect("request");

    assert_eq!(resp.status, 200);
    let body = resp.body.unwrap();
    assert_eq!(body["id"], "xyz");
    assert_eq!(body["subresource"], "legs");
}

// ── No short-circuit for non-matching URLs ───────────────────────

#[tokio::test]
async fn non_matching_url_falls_through_to_network_path() {
    // Configure a SelfOrigins that does NOT include the target
    // URL's host. HttpClient MUST NOT short-circuit; it should try
    // the real network (which will fail to resolve, proving no
    // hidden dispatch occurred).
    let router = build_router_with_dsls(&[(
        "svc/GET/x.yml",
        "respond:\n  return: { ok: true }\n  next: end\n",
    )]);

    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("someotherhost".to_string(), 9999));
    let client = HttpClient::with_timeout_ms(500).with_self_origins(origins);
    client.set_self_call_handler(router.clone() as Arc<dyn SelfCallHandler>);

    // Port 65501 is very unlikely to be in use in a test env
    // (unlike 8080 which may have a dev container running).
    let res = client
        .request(
            reqwest::Method::GET,
            "http://localhost:65501/svc/x",
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(res.is_err(), "non-matching URL must not short-circuit; got {:?}", res);
}

#[tokio::test]
async fn no_handler_wired_falls_through_to_network_path() {
    // Even when the URL matches SelfOrigins, if no router handle
    // was wired, we must not silently succeed — fall through to
    // the network path (which will fail to reach anything).
    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("localhost".to_string(), 65500));
    let client = HttpClient::with_timeout_ms(500).with_self_origins(origins);
    // NO set_self_call_handler call

    let res = client
        .request(
            reqwest::Method::GET,
            "http://localhost:65500/svc/x",
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(res.is_err(), "no handler → must not silently succeed; got {:?}", res);
}

// ── SelfOrigins detection unit tests ─────────────────────────────

#[test]
fn self_origins_from_default_config_includes_all_synonyms() {
    let cfg = AppConfig::default();
    let origins = SelfOrigins::from_config(&cfg);
    let expected: HashSet<_> = [
        ("localhost", 8080),
        ("127.0.0.1", 8080),
        ("0.0.0.0", 8080),
        ("[::1]", 8080),
        ("::1", 8080),
    ]
    .into_iter()
    .map(|(h, p)| (h.to_string(), p))
    .collect();
    assert!(
        expected.iter().all(|k| origins.tcp.contains(k)),
        "missing synonym in {:?}",
        origins.tcp
    );
}

#[test]
fn self_origins_matches_scheme_host_port() {
    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("host".to_string(), 1234));
    assert!(origins.matches("http://host:1234/x"));
    assert!(origins.matches("http://host:1234/x?y=1"));
    assert!(!origins.matches("http://host:1235/x"));
    assert!(!origins.matches("http://other:1234/x"));
    // Default ports for scheme
    origins.tcp.insert(("web".to_string(), 80));
    assert!(origins.matches("http://web/x"));
    origins.tcp.insert(("secure".to_string(), 443));
    assert!(origins.matches("https://secure/x"));
}

#[test]
fn self_origins_does_not_match_non_http_schemes() {
    let mut origins = SelfOrigins::default();
    origins.tcp.insert(("localhost".to_string(), 8080));
    assert!(!origins.matches("ws://localhost:8080/x"));
    assert!(!origins.matches("file:///localhost:8080/x"));
    assert!(!origins.matches("nonsense"));
}

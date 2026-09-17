//! h2ck.me v1 T-31 — JSON deep-nesting positive control.
//!
//! serde_json rejects JSON input past a ~128-layer implicit
//! recursion limit. The 2026-09-11 audit sweep (BREAK-TESTS-OWASP-
//! PROBES-v1 §F-PR-5) probed the framework with `{"a":{"a":{...
//! depth=100 ...}}}` and got a clean `200` (below the limit) and
//! `{... depth=1000 ...}` and got a clean `400` (above the limit).
//! Safe today — no additional cap needed in Ruuter itself. But
//! there is no regression test pinning this behaviour, so a
//! future `serde_json` upgrade that raises or removes the limit
//! could silently regress the fleet without anyone noticing.
//!
//! This file is the positive-control pin. It does NOT modify any
//! production code. It asserts two adjacent inputs — one below the
//! limit, one above — round-trip to the expected HTTP status. If
//! serde_json ever changes the depth ceiling, the wide-limit test
//! ("depth 200 rejected") will start returning 200 and this file
//! will fail loudly, giving whoever ran the upgrade a chance to
//! decide whether to raise or lower the cap explicitly.
//!
//! Numbers picked per §F-PR-5:
//! - depth 100 → below the ~128 default → 200 accepted.
//! - depth 200 → above the ~128 default → 400 rejected with an
//!   error body pointing at JSON parse failure.
//!
//! We do NOT probe the exact serde_json limit (128 vs 132 etc.);
//! adjacent-limit fuzzing is out of scope. The pin is only that a
//! reasonable "well under" number succeeds and a reasonable "well
//! over" number fails.

// Test-fixture AppConfig assembly follows the shape used by other
// integration tests. See tests/security_new_probes.rs for rationale.
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
use tokio::net::TcpListener;

fn uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, seq)
}

fn build_router(cfg: AppConfig, files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-jsondepth-{}", uuid()));
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
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

/// Build a JSON body of the shape `{"a":{"a":...{"a":1}...}}` with
/// exactly `depth` nested objects (the innermost object is
/// `{"a":1}`). Depth 1 = `{"a":1}`, depth 2 = `{"a":{"a":1}}`, etc.
fn nested_json_body(depth: usize) -> String {
    // Iterative construction — a recursive helper would itself
    // hit the Rust call-stack limit on inputs that exercise
    // serde_json's parser.
    let mut s = String::with_capacity(depth * 5 + 8);
    for _ in 0..depth {
        s.push_str("{\"a\":");
    }
    s.push('1');
    for _ in 0..depth {
        s.push('}');
    }
    s
}

/// Depth 100 is comfortably below serde_json's implicit ~128
/// recursion limit. The DSL simply echoes back; a 200 confirms the
/// framework parsed the body and dispatched normally.
#[tokio::test]
async fn depth_100_accepted_below_serde_json_limit() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            r#"
echo:
  return: { ok: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;

    let body = nested_json_body(100);
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "depth 100 is below serde_json's implicit ~128 limit and must be accepted; \
         got {} — did serde_json's recursion cap change? See T-31.",
        resp.status()
    );
    let parsed: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(parsed["response"]["ok"], serde_json::json!(true));
}

/// Depth 200 is comfortably above serde_json's implicit ~128
/// recursion limit. Ruuter's incoming body deserializer must reject
/// with a 4xx (JSON parse error). A 200 here would mean the parser
/// silently accepted a pathologically deep body — regression on the
/// implicit stack safety serde_json provides.
#[tokio::test]
async fn depth_200_rejected_above_serde_json_limit() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            r#"
echo:
  return: { ok: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;

    let body = nested_json_body(200);
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "depth 200 exceeds serde_json's implicit ~128 recursion limit and MUST be \
         rejected with 4xx; got {}. If this test starts returning 2xx, serde_json \
         has raised or removed its default recursion cap — decide whether Ruuter \
         should ship its own explicit depth cap before landing that upgrade. \
         See T-31.",
        status
    );
}

/// Sanity: a modestly-nested body is not accidentally rejected.
/// Guards against a future overshoot where we clamp too aggressively.
#[tokio::test]
async fn depth_10_accepted_trivial_shape() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            r#"
echo:
  return: { ok: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;

    let body = nested_json_body(10);
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

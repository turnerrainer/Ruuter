//! Security hardening test suite — automated tests that exercise
//! Ruuter-on-Rust from the outside as a hostile client would.
//!
//! Every test spins the real router on a random localhost port via
//! `axum::serve` and drives it with `reqwest`, so the entire request-
//! processing chain (Axum middleware → CSRF → CORS → method allowlist →
//! idempotency → DSL engine → response headers) is under test. Unit
//! tests on individual modules cannot catch regressions in wiring
//! order; these do.
//!
//! The suite is organised by attack surface:
//!
//! - `idempotency_*` — Idempotency-Key replay semantics (dedup key
//!   composition, cross-tenant collision risk, oracle probing).
//! - `ssrf_*` — Outbound HTTP allowlist bypasses (prefix confusion,
//!   self-call bypass of the disabled-outbound switch, UDS-alias
//!   escape).
//! - `path_*` — Path traversal and DSL-key resolution edge cases.
//! - `header_*` — CRLF injection into response headers via DSL and
//!   via response_default_headers config.
//! - `body_*` — Request body size caps, malformed JSON handling.
//! - `csrf_*` — Origin/Referer allow-list edge cases (case, port,
//!   trailing slash).
//! - `method_*` — Method allow-list bypass attempts.
//! - `dsl_*` — DSL loader / scripting sandbox behaviour visible over
//!   the wire (JS DoS caps, iterate cap enforcement).
//! - `logging_*` — Log injection through user-controlled `log` step
//!   input and reflected traceparent.
//! - `state_*` — Concurrent state races that let two callers get the
//!   same reserved value.
//! - `ws_*` — WS server abuse (giant frames, non-JSON frames).
//! - `misc_*` — Trace header spoofing, X-Forwarded-For trust,
//!   admin endpoints exposure.
//!
//! Findings map:
//! - S1 = Idempotency-Key body-hash omission (cross-caller replay).
//! - S2 = SSRF allowlist prefix substring bypass.
//! - S3 = Self-call bypasses `internal_requests.disabled`.
//! - S4 = X-Forwarded-For adopted verbatim as `origin`.
//! - S5 = Idempotency oracle via `Idempotency-Replayed: true` header.
//! - S6 = SSRF check does not re-run on redirects.
//! - S7 = /health leaks framework version (fingerprinting).
//! - S8 = Vulnerable/unmaintained dependencies present in Cargo.lock
//!   (RUSTSEC-2025-0003 fast-float, RUSTSEC-2025-0068 serde_yml).
//!
//! Each test doc-comment says which finding it exercises.

// Test-fixture AppConfig assembly. See tests/trigger_dispatch.rs for
// rationale.
#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{
    AppConfig, CsrfConfig, IncomingRequestsConfig, InternalRequestsConfig,
    OptimisticConcurrencyConfig,
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

// ── Shared helpers ─────────────────────────────────────────────────

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
    let tmp = std::env::temp_dir().join(format!("ruuter-sec-hard-{}", uuid()));
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

/// A DSL that echoes the caller's inbound JSON body back verbatim
/// under `echo:` and includes `Idempotency-Key`-relevant fields.
/// Used by the idempotency oracle / cross-caller tests.
const ECHO_BODY_DSL: &str = r#"
respond:
  return: { echo: "${incoming.body}", got_key: "${incoming.headers['idempotency-key']}" }
  next: end
"#;

// ── Idempotency: replay-key composition ────────────────────────────

/// **Finding S1** — the dedup key is `sha256(idempotency_key | method |
/// project | path)`. It does NOT include the request body. Two callers
/// posting different bodies with the same Idempotency-Key on the same
/// endpoint replay the FIRST caller's response — cross-caller data
/// leak or intent hijack.
///
/// **Post-v1.0** — framework-level Idempotency-Key handling was
/// removed (h2ck.me findings S1 + S5). This test pins the new
/// contract: two callers with the same key on the same endpoint
/// each get THEIR OWN body echoed back — no cross-caller replay,
/// no `Idempotency-Replayed` header ever emitted. DSL authors who
/// want idempotency implement it via `state.get`/`state.set` with
/// their own identity + body-hash keys.
#[tokio::test]
async fn idempotency_no_cross_caller_replay_after_framework_removal() {
    let cfg = AppConfig::default();
    let router = build_router(cfg, &[("svc/POST/echo.yml", ECHO_BODY_DSL)]);
    let port = serve(router).await;
    let c = client();

    let key = "shared-key-1";
    let a: serde_json::Value = c
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "owner": "alice", "amount": 100 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a["response"]["echo"]["owner"], "alice");

    let resp_b = c
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "owner": "eve", "amount": 999 }))
        .send()
        .await
        .unwrap();
    assert!(
        resp_b.headers().get("idempotency-replayed").is_none(),
        "framework must not emit Idempotency-Replayed after v1.0 removal"
    );
    let b: serde_json::Value = resp_b.json().await.unwrap();
    assert_eq!(
        b["response"]["echo"]["owner"], "eve",
        "S1 remediated: caller B sees its OWN body, not A's"
    );
}

/// **Post-v1.0** — `Idempotency-Replayed` header (h2ck.me S5 oracle)
/// is never emitted by the framework, so no membership probe is
/// possible from the outside.
#[tokio::test]
async fn idempotency_replayed_header_never_emitted_after_removal() {
    let cfg = AppConfig::default();
    let router = build_router(cfg, &[("svc/POST/echo.yml", ECHO_BODY_DSL)]);
    let port = serve(router).await;

    let key = "oracle-probe-key";
    let victim = client();
    victim
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "from": "victim" }))
        .send()
        .await
        .unwrap();

    let attacker = client();
    let probe = attacker
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "from": "attacker-probe" }))
        .send()
        .await
        .unwrap();
    assert!(
        probe.headers().get("idempotency-replayed").is_none(),
        "S5 remediated: framework never emits Idempotency-Replayed"
    );
}

/// **Post-v1.0** — with no framework-level idempotency at all, both
/// calls run the DSL independently. This pins that regression: an
/// accidental re-introduction of the middleware would trip this.
#[tokio::test]
async fn idempotency_key_has_no_framework_effect() {
    let cfg = AppConfig::default();
    let router = build_router(cfg, &[("svc/POST/echo.yml", ECHO_BODY_DSL)]);
    let port = serve(router).await;
    let c = client();

    let key = "k1";
    let first = c
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "who": "a" }))
        .send()
        .await
        .unwrap();
    assert!(first.headers().get("idempotency-replayed").is_none());
    let first_body: serde_json::Value = first.json().await.unwrap();
    assert_eq!(first_body["response"]["echo"]["who"], "a");

    let second = c
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("idempotency-key", key)
        .json(&serde_json::json!({ "who": "b" }))
        .send()
        .await
        .unwrap();
    assert!(
        second.headers().get("idempotency-replayed").is_none(),
        "framework must not emit Idempotency-Replayed"
    );
    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(
        body["response"]["echo"]["who"], "b",
        "second call runs its own DSL"
    );
}

// ── SSRF: outbound allowlist bypasses ──────────────────────────────

/// **Finding S2 — PINNED FIX** — `check_ssrf` used to use `starts_with`
/// against `allowed_url_prefixes`, so a bare-origin entry like
/// `http://api.example.com` (no trailing slash) accepted a lookalike
/// `http://api.example.com.evil.tld/x`. Post-v1.0, bare-origin entries
/// require an EXACT scheme+host+port match; path-scoped entries still
/// use `starts_with` but only after the origin matches exactly.
///
/// Pins the fix: an allowlist entry of `http://127.0.0.1` must NOT
/// admit a request to `http://127.0.0.1.attacker.tld/foo`. The SSRF
/// block should fire BEFORE the DNS attempt, so the error carries
/// `allowed_urls`, not a DNS/connect failure.
#[tokio::test]
async fn ssrf_prefix_check_is_substring_and_leaks_to_lookalike_domain() {
    // Start a "malicious upstream" on 127.0.0.1 to observe the request.
    let victim = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let victim_port = victim.local_addr().unwrap().port();
    let (marker_tx, marker_rx) = std::sync::mpsc::channel::<()>();
    tokio::spawn(async move {
        // Accept once, respond with a hello, signal that we were hit.
        if let Ok((mut stream, _)) = victim.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n{\"pwned\":1}")
                .await;
            let _ = marker_tx.send(());
        }
    });

    // The operator "meant" api.example.com but wrote a bare prefix.
    // We use `127.0.0.1` as the substring so we can point the attack
    // at the local listener.
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        // Note: no trailing slash — this is the misconfiguration.
        allowed_urls: vec!["http://127.0.0.1".to_string()],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}.attacker.tld/foo"
  result: r
  next: reply
reply:
  return: {{ tried: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/leak.yml", dsl.as_str())]);
    let port = serve(router).await;

    // The lookalike domain won't resolve so the outbound will fail —
    // but the SSRF check should have blocked it BEFORE the DNS attempt.
    // We assert the failure is a "url not in ..." error (SSRF-block),
    // not a DNS/connection error (SSRF permitted, network denied).
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/leak", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    // S2 PINNED FIX: the SSRF check now blocks the request before
    // the network is touched. The error must reference the allowlist
    // (not a DNS/transport failure), and the victim listener must
    // never have been contacted.
    assert!(
        err.contains("allowed_urls") || err.contains("not in"),
        "S2 fix: SSRF check must have blocked the lookalike origin — err was: {}",
        err
    );
    assert!(
        marker_rx.try_recv().is_err(),
        "S2 fix: lookalike origin reached the victim listener — SSRF check leaked"
    );
}

/// **Finding S3 — PINNED FIX** — `internal_requests.disabled = true`
/// used to only block reqwest calls; the task-044 self-call short-
/// circuit and UDS transports slipped past it. Post-v1.0, the
/// `outbound_disabled` guard runs at the very top of
/// `HttpClient::request`, before self-call/UDS dispatch, so an
/// operator's "no outbound HTTP" decision holds regardless of
/// transport.
///
/// Pins the fix: a DSL that self-calls its own route via
/// `http.get` must fail with an `outbound HTTP is disabled` error
/// when the flag is on.
#[tokio::test]
async fn ssrf_self_call_bypasses_outbound_disabled() {
    // The router must have a real listener bound so the self-origin
    // lookup finds a match. We give it two routes: `/svc/side-effect`
    // performs an observable action; `/svc/attack` calls the first via
    // http.get with `internal_requests.disabled = true`.
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: true,
        allowed_ips: vec![],
        allowed_urls: vec![],
        block_private_networks: false,
    };
    // Force the router's SelfOrigins to know about 127.0.0.1:<port>.
    // The router picks up `config.port` for self-origin registration,
    // so we set the port explicitly then bind our listener on it.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    cfg.port = port;

    let attack_dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/svc/side-effect"
  result: r
  next: reply
reply:
  return: {{ side: "${{r.response.body.hit}}" }}
  next: end
"#,
        port
    );
    let files: &[(&str, &str)] = &[
        (
            "svc/GET/side-effect.yml",
            "reply:\n  return: { hit: pwned }\n  next: end\n",
        ),
        ("svc/GET/attack.yml", attack_dsl.as_str()),
    ];
    // Build router but re-use the pre-bound listener so its port
    // matches the SelfOrigins map exactly.
    let tmp = std::env::temp_dir().join(format!("ruuter-sec-self-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg2 = cfg.clone();
    cfg2.config_path = tmp;
    let loader = DslLoader::new(cfg2.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let http_client = HttpClient::new(&cfg2);
    let engine = StepEngine::new(http_client.clone())
        .with_ws_registry(ws.clone())
        .with_dsls(shared.clone());
    let router = Arc::new(DslRouter::from_arc(
        shared,
        loaded.guards,
        cfg2,
        StateStore::new(),
        ws,
        engine,
    ));
    // Wire the self-call handler — same as main.rs does at boot.
    http_client.set_self_call_handler(router.clone());
    let app = router.build_axum_router_from_arc();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/attack", port))
        .send()
        .await
        .unwrap();
    // S3 PINNED FIX: the outbound guard runs before the self-call
    // dispatch, so the attack DSL must fail with a 500 whose body
    // carries the "outbound HTTP is disabled" error. `side` must NOT
    // be `pwned` — that would mean the self-call fired anyway.
    assert_eq!(resp.status(), 500, "S3 fix: attack must fail, not succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("outbound HTTP is disabled"),
        "S3 fix: expected outbound-disabled error, got body: {}",
        body
    );
}

/// Regression: with outbound disabled and NO self-origin match, the
/// http step must fail. Guards the existing (correct) network path.
#[tokio::test]
async fn ssrf_outbound_disabled_still_blocks_non_self_urls() {
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
    url: "http://192.0.2.99:12345/should-be-blocked"
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
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("outbound HTTP is disabled"),
        "expected outbound-disabled error; got: {}",
        err
    );
}

// ── Path traversal / DSL-key resolution ────────────────────────────

/// Traversal via `..` in the request path must not leak into the DSL
/// lookup namespace. Because `reqwest` normalises `..` client-side,
/// we send the request over a raw TCP socket to control exactly
/// what appears on the wire.
async fn raw_get(port: u16, raw_path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        raw_path
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let raw = String::from_utf8_lossy(&buf).to_string();
    // Split status line off, then find start-of-body.
    let status = raw
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body = raw
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .trim_end_matches(char::from(0))
        .to_string();
    (status, body)
}

/// Traversal via literal `..` in the request-target must not let
/// project A's caller reach project B. Uses raw TCP so `reqwest`'s
/// client-side URL normalisation doesn't hide the bug.
#[tokio::test]
async fn path_traversal_dotdot_does_not_pivot_between_projects() {
    let router = build_router(
        AppConfig::default(),
        &[
            (
                "svc-a/GET/public.yml",
                "reply:\n  return: { where: a }\n  next: end\n",
            ),
            (
                "svc-b/GET/secret.yml",
                "reply:\n  return: { where: b, secret: true }\n  next: end\n",
            ),
        ],
    );
    let port = serve(router).await;

    // Sanity: the direct hit works.
    let (ok_status, ok_body) = raw_get(port, "/svc-a/public").await;
    assert_eq!(ok_status, 200);
    assert!(ok_body.contains("\"a\""));

    // Traversal attempt. Whatever the response, `where: b` must not
    // appear — that would be the pivot.
    let (bad_status, bad_body) = raw_get(port, "/svc-a/../svc-b/secret").await;
    assert!(
        !bad_body.contains("\"b\""),
        "path traversal reached svc-b (status {}): {}",
        bad_status,
        bad_body
    );
}

/// Encoded traversal (`%2e%2e`) sent literally on the wire must not
/// resolve into another project either.
#[tokio::test]
async fn path_traversal_percent_encoded_does_not_pivot() {
    let router = build_router(
        AppConfig::default(),
        &[
            (
                "svc-a/GET/public.yml",
                "reply:\n  return: { where: a }\n  next: end\n",
            ),
            (
                "svc-b/GET/secret.yml",
                "reply:\n  return: { where: b, secret: true }\n  next: end\n",
            ),
        ],
    );
    let port = serve(router).await;
    let (status, body) = raw_get(port, "/svc-a/%2e%2e/svc-b/secret").await;
    assert!(
        !body.contains("\"b\""),
        "%2e%2e traversal reached svc-b (status {}): {}",
        status,
        body
    );
}

/// Sanity: the OpenAPI endpoint is not shadowed by a hostile DSL at
/// `_/openapi.json`. v0.9.11 (h2ck.me M1): the OpenAPI handler moved
/// to `admin_router()`, so on the PUBLIC router the DSL-side path
/// `/_/openapi.json` legitimately falls through to the DSL dispatch
/// fallback (which then either 404s if no `_/` project exists or
/// runs the shadowing DSL as the "_" project would). What must NOT
/// happen is a hostile DSL at that path masquerading as the OpenAPI
/// spec to callers hitting the ADMIN endpoint — the admin router
/// has an exact-match route that outranks the fallback.
#[tokio::test]
async fn admin_route_not_shadowed_by_dsl_at_same_path() {
    let router = std::sync::Arc::new(build_router(
        AppConfig::default(),
        &[(
            "_/GET/openapi.json.yml",
            "reply:\n  return: { pwned: true }\n  next: end\n",
        )],
    ));
    use tower::ServiceExt;
    let resp = router
        .admin_router()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/_/openapi.json")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body.get("pwned").is_none(),
        "the DSL shadowed the OpenAPI handler on the admin router"
    );
    assert!(
        body.get("openapi").is_some() || body.get("info").is_some(),
        "admin_router() response was not the OpenAPI spec: {}",
        body
    );
}

// ── Header injection / CRLF via return step ────────────────────────

/// The DSL author can build response headers from user input:
/// `headers: { "X-Echo": "${incoming.body.value}" }`. If the value
/// contains `\r\n`, the framework must reject it — otherwise a caller
/// can inject a second header (`Set-Cookie: session=stolen`).
#[tokio::test]
async fn header_injection_via_crlf_in_return_header_is_dropped() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            r#"
reply:
  return: { ok: true }
  status: 200
  headers:
    x-echo: "${incoming.body.value}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .json(&serde_json::json!({
            "value": "harmless\r\nSet-Cookie: pwned=1"
        }))
        .send()
        .await
        .unwrap();
    // Injection sanity: response must not have a Set-Cookie header
    // from the CRLF-appended segment, and the X-Echo header (if
    // present) must not contain a literal CR/LF.
    assert!(
        resp.headers().get("set-cookie").is_none(),
        "CRLF injection produced a Set-Cookie header"
    );
    if let Some(x) = resp.headers().get("x-echo") {
        let s = x.to_str().unwrap_or("");
        assert!(
            !s.contains('\r') && !s.contains('\n'),
            "x-echo contains raw CR/LF — header split possible: {:?}",
            s
        );
    }
}

/// Same threat via `response_default_headers` config. Malformed
/// header names / CR-LF values from operator config must be dropped,
/// not silently corrupt every response.
#[tokio::test]
async fn response_default_headers_ignore_invalid_pairs() {
    let mut cfg = AppConfig::default();
    cfg.response_default_headers
        .insert("valid-header".to_string(), "value".to_string());
    cfg.response_default_headers
        .insert("invalid header with space".to_string(), "value".to_string());
    cfg.response_default_headers.insert(
        "crlf".to_string(),
        "value\r\nSet-Cookie: pwned=1".to_string(),
    );
    let router = build_router(
        cfg,
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/ping", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("valid-header")
            .map(|v| v.to_str().unwrap().to_string()),
        Some("value".to_string()),
        "valid default header was dropped"
    );
    assert!(
        resp.headers().get("set-cookie").is_none(),
        "CRLF-carrying default header injected an extra header"
    );
    assert!(
        resp.headers().get("invalid header with space").is_none(),
        "invalid header name was accepted"
    );
}

// ── Body limits & malformed input ──────────────────────────────────

/// The router accepts up to 16 MiB of body. A request over the cap
/// must return 400 without OOM'ing the server. This test uses a 32 MiB
/// body — well over the cap, but small enough to be cheap.
#[tokio::test]
async fn body_over_16_mib_is_rejected() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/sink.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let big = "x".repeat(20 * 1024 * 1024);
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/sink", port))
        .header("content-type", "application/json")
        .body(format!("{{\"pad\":\"{}\"}}", big))
        .send()
        .await;
    match resp {
        Ok(r) => {
            // Anything except 2xx is fine — 400 is expected, but a
            // 413 or a broken-pipe surfaced as some other 4xx/5xx
            // is also acceptable defence.
            assert!(
                r.status().as_u16() >= 400,
                "20 MiB body must not be accepted"
            );
        }
        Err(_e) => {
            // Connection reset by peer is also fine — the point is
            // the server did not accept it.
        }
    }
}

/// A giant JSON body with declared `Content-Type: application/json`
/// containing unbalanced braces must return 400, not 500 or a
/// crashing thread.
#[tokio::test]
async fn body_malformed_json_returns_400_not_500() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/sink.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/sink", port))
        .header("content-type", "application/json")
        .body("{\"a\":".to_string() + &"{".repeat(1000))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── CSRF: Origin/Referer edge cases ────────────────────────────────

/// Origin comparison must be exact — `https://ok.example.com` on the
/// allow-list must NOT match `https://ok.example.com.evil.tld`.
#[tokio::test]
async fn csrf_origin_prefix_lookalike_is_rejected() {
    let mut cfg = AppConfig::default();
    cfg.csrf = CsrfConfig {
        allowed_origins: vec!["https://ok.example.com".to_string()],
        enforce_on_methods: vec!["POST".into()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/POST/act.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/act", port))
        .header("origin", "https://ok.example.com.evil.tld")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "lookalike origin passed CSRF check");
}

/// Trailing-slash and case variations must not sneak past the allow-
/// list. `HTTPS://` vs `https://` are byte-different but browsers
/// only ever send lowercase scheme in Origin — a mismatch is fine to
/// reject.
#[tokio::test]
async fn csrf_origin_trailing_slash_variant_is_rejected() {
    let mut cfg = AppConfig::default();
    cfg.csrf = CsrfConfig {
        allowed_origins: vec!["https://ok.example.com".to_string()],
        enforce_on_methods: vec!["POST".into()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/POST/act.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/act", port))
        .header("origin", "https://ok.example.com/")
        .send()
        .await
        .unwrap();
    // If the allow-list stores `https://ok.example.com` and the
    // browser sends `https://ok.example.com/`, they don't match. The
    // safe behaviour is to reject. We assert rejection; if this
    // trips (framework starts normalising origins), remove the test.
    assert_eq!(
        resp.status(),
        403,
        "origin with trailing slash was accepted — check for over-normalisation"
    );
}

/// A Referer without an Origin whose scheme+host+port match the
/// allow-list must pass — the fallback is intentional and documented
/// (server-side navigation, non-JS clients). But if the Referer is on
/// a NON-allowed origin, POST must be rejected.
#[tokio::test]
async fn csrf_disallowed_referer_without_origin_is_rejected() {
    let mut cfg = AppConfig::default();
    cfg.csrf = CsrfConfig {
        allowed_origins: vec!["https://ok.example.com".to_string()],
        enforce_on_methods: vec!["POST".into()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/POST/act.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/act", port))
        .header("referer", "https://evil.example.com/foo")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

// ── Method allow-list edge cases ───────────────────────────────────

/// Method matching uses `eq_ignore_ascii_case`. `PATCH` on an allow-
/// list of `["GET"]` must still 405 regardless of case.
#[tokio::test]
async fn method_allowlist_case_insensitive_reject() {
    let mut cfg = AppConfig::default();
    cfg.incoming_requests = IncomingRequestsConfig {
        allowed_method_types: vec!["GET".into()],
        headers: HashMap::new(),
    };
    let router = build_router(
        cfg,
        &[(
            "svc/PATCH/act.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .patch(format!("http://127.0.0.1:{}/svc/act", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

// ── Optimistic-concurrency: If-Match enforcement ───────────────────

/// When `optimistic_concurrency.require_if_match = true`, a PUT
/// without `If-Match` gets 428. This defends against blind overwrite
/// after an ETag-mismatch race. Regression guard for the wiring order.
#[tokio::test]
async fn if_match_required_returns_428_when_missing() {
    let mut cfg = AppConfig::default();
    cfg.optimistic_concurrency = OptimisticConcurrencyConfig {
        require_if_match: true,
        enforce_on_methods: vec!["PUT".into()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/PUT/thing.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .put(format!("http://127.0.0.1:{}/svc/thing", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 428);
}

// ── Scripting: sandbox DoS caps ────────────────────────────────────

/// A DSL author (or a malicious one who slipped a bad expression past
/// review) using unbounded recursion must be capped by Boa's
/// recursion limit and surface as a 500 — never hang the tokio
/// worker.
///
/// We test recursion rather than `while(true){}` because Boa's loop-
/// iteration cap defaults to 1M and interpreted iteration of an
/// empty body takes several seconds even under the cap. Recursion
/// hits `max_stack_size` (default 400) in milliseconds.
///
/// Boa-only: QuickJS has a different stack-cap surface (see task 036
/// backlog) so unbounded recursion under rquickjs currently
/// overflows the OS thread stack rather than surfacing as a 500.
/// Track the QuickJS gap in the scripting-quickjs backlog rather than
/// letting it crash CI here.
#[cfg(feature = "scripting-boa")]
#[tokio::test]
async fn scripting_runaway_recursion_hits_stack_cap() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/hang.yml",
            r#"
reply:
  return:
    value: "${(function f(){return f();})()}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let start = std::time::Instant::now();
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client()
            .get(format!("http://127.0.0.1:{}/svc/hang", port))
            .send(),
    )
    .await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stack recursion cap did not stop the runaway DSL (elapsed {:?})",
        elapsed
    );
    let resp = resp.expect("cap did not fire in 5s").expect("send err");
    assert!(
        resp.status().as_u16() >= 400,
        "runaway recursion returned {} — cap not enforced end-to-end",
        resp.status()
    );
}

/// Complements the recursion test: a JS `while(true){}` submitted
/// through the script engine directly must abort on the loop
/// iteration cap. Runs at unit level to sidestep the process-wide
/// `install_default_limits` OnceCell — we pass a fresh, tight limit
/// via `BoaScriptEngine::with_limits`.
///
/// Kept `#[cfg(feature = "scripting-boa")]` because the QuickJS
/// backend has a different cap surface.
#[cfg(feature = "scripting-boa")]
#[test]
fn scripting_while_true_hits_loop_iteration_cap_unit_level() {
    use ruuter_on_rust::context::ExecutionContext;
    use ruuter_on_rust::scripting::{boa::BoaScriptEngine, ScriptLimits};
    let ctx = ExecutionContext::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        "test".to_string(),
    );
    let engine = BoaScriptEngine::with_limits(ScriptLimits {
        max_loop_iterations: 10_000,
        max_stack_size: 400,
    });
    let start = std::time::Instant::now();
    let out = engine.evaluate(
        &serde_json::Value::String("${while(true){}}".to_string()),
        &ctx,
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "loop cap didn't fire (elapsed {:?})",
        elapsed
    );
    assert!(
        out.is_err(),
        "runaway loop returned Ok — cap did not surface as an error"
    );
}

/// The Boa sandbox must NOT expose Node.js globals (`process`,
/// `require`, `Buffer`, `global`). Test that these are `undefined` /
/// `ReferenceError` when a DSL references them.
#[tokio::test]
async fn scripting_no_node_globals_exposed() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/probe.yml",
            r#"
probe:
  assign:
    has_process: "${typeof process}"
    has_require: "${typeof require}"
    has_buffer: "${typeof Buffer}"
    has_global: "${typeof global}"
  next: reply
reply:
  return:
    has_process: "${has_process}"
    has_require: "${has_require}"
    has_buffer: "${has_buffer}"
    has_global: "${has_global}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/svc/probe", port))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for key in ["has_process", "has_require", "has_buffer", "has_global"] {
        assert_eq!(
            body["response"][key], "undefined",
            "Node global {} leaked into DSL scripting sandbox",
            key
        );
    }
}

/// The scripting sandbox must not be able to read filesystem paths.
/// Neither Boa nor QuickJS ships `fs`, but a probe protects against a
/// future binding accidentally being wired in.
///
/// `typeof <undeclared>` returns the string `"undefined"` without
/// throwing — using bare identifiers keeps the probe engine-agnostic
/// (`this` behaves differently under strict/non-strict eval).
#[tokio::test]
async fn scripting_no_filesystem_binding() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/fs.yml",
            r#"
probe:
  return:
    fs: "${typeof fs}"
    read: "${typeof readFileSync}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/svc/fs", port))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["response"]["fs"], "undefined");
    assert_eq!(body["response"]["read"], "undefined");
}

// ── Iterate: DoS cap ───────────────────────────────────────────────

/// A DSL that constructs a giant list from user input must not
/// iterate unboundedly. Default cap is 10_000; a request generating
/// 100_000 items must be rejected (500 with a max_items message).
#[tokio::test]
async fn iterate_over_default_cap_rejects() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/blast.yml",
            r#"
blast:
  iterate:
    over: "${Array.from({length: 100000}, (_,i)=>i)}"
    as: item
    do:
      - assign: { last: "${item}" }
  next: reply
reply:
  return: { done: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .post(format!("http://127.0.0.1:{}/svc/blast", port))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().as_u16() >= 400,
        "100k-item iterate must be capped, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("max_items"),
        "expected max_items error, got: {}",
        err
    );
}

// ── Log-step injection ─────────────────────────────────────────────

/// `log:` message evaluation must not double-evaluate substituted
/// values. A body value of `${1+1}` submitted by the caller must NOT
/// be re-evaluated after substitution — the log should read the
/// literal string, not `2`.
#[tokio::test]
async fn log_step_does_not_reevaluate_substituted_values() {
    // The point of this test is to catch a regression that would
    // introduce a "user body -> executed JS" pipeline. If the log
    // step ever re-evaluates the substituted expression, the DSL run
    // would evaluate `1+1` = 2, and the response (which mirrors the
    // logged value) would show `2` instead of the literal string.
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/mirror.yml",
            r#"
capture:
  assign:
    logged: "user said: ${incoming.body.msg}"
  next: reply
reply:
  return:
    logged: "${logged}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .post(format!("http://127.0.0.1:{}/svc/mirror", port))
        .json(&serde_json::json!({ "msg": "${1+1}" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body["response"]["logged"], "user said: ${1+1}",
        "user-supplied ${{...}} was re-evaluated by the framework — server-side template injection"
    );
}

// ── traceparent handling ───────────────────────────────────────────

/// A malformed `traceparent` header must be echoed as-is (per the
/// current implementation), but the derived `X-Trace-Id` must be
/// absent — extracting a 32-hex trace_id from garbage would falsify
/// telemetry.
#[tokio::test]
async fn traceparent_malformed_does_not_produce_bad_trace_id() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("traceparent", "not-a-valid-traceparent")
        .send()
        .await
        .unwrap();
    // traceparent is echoed verbatim — that's the contract.
    let tp = resp
        .headers()
        .get("traceparent")
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(tp, Some("not-a-valid-traceparent".to_string()));
    // But the derived X-Trace-Id must not be present (or must be a
    // 32-hex string, if the framework decided to synthesise a fresh
    // one on malformed input).
    if let Some(xtid) = resp.headers().get("x-trace-id") {
        let s = xtid.to_str().unwrap();
        assert_eq!(
            s.len(),
            32,
            "x-trace-id derived from malformed traceparent is not 32 hex chars: {:?}",
            s
        );
        assert!(
            s.chars().all(|c| c.is_ascii_hexdigit()),
            "x-trace-id contains non-hex chars: {:?}",
            s
        );
    }
}

/// A traceparent header of the wrong length must not be truncated
/// into a valid-looking trace id. Attackers who control the incoming
/// traceparent must not be able to steer server-side trace_id values.
#[tokio::test]
async fn traceparent_wrong_length_does_not_produce_trace_id() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    // 8 hex chars in the trace_id slot — not 32.
    let bad_tp = "00-deadbeef-1122334455667788-01";
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/ping", port))
        .header("traceparent", bad_tp)
        .send()
        .await
        .unwrap();
    if let Some(xtid) = resp.headers().get("x-trace-id") {
        let s = xtid.to_str().unwrap();
        assert_ne!(
            s, "deadbeef",
            "server extracted a bogus 8-char x-trace-id from a malformed traceparent"
        );
    }
}

// ── X-Forwarded-For trust ──────────────────────────────────────────

/// **Finding S4 — PINNED FIX** — the router used to promote
/// `X-Forwarded-For` (or `X-Real-IP`) verbatim into
/// `incoming.origin`, letting any direct caller spoof the origin
/// values downstream code keys off (audit logs, rate limiters,
/// self-call bookkeeping). Post-v1.0, XFF adoption is gated on
/// `config.proxy.trusted`: the promotion only happens when the
/// direct TCP peer's IP is in the operator's trusted-proxy list.
///
/// This test pins both halves of the contract:
///   * `incoming.headers["x-forwarded-for"]` still shows the raw
///     header (a DSL that wants to log the client claim can),
///   * `incoming.origin` reflects the socket peer, NOT the XFF value,
///     because the test peer (a loopback reqwest client) is not on
///     any trusted list.
#[tokio::test]
async fn xff_from_untrusted_peer_is_not_adopted_as_origin() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/probe.yml",
            r#"
reply:
  return:
    xff: "${incoming.headers['x-forwarded-for']}"
    origin: "${incoming.origin}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/svc/probe", port))
        .header("x-forwarded-for", "10.0.0.99, 8.8.8.8, ::1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Raw header remains visible for any DSL that wants to log or
    // inspect the client's claim.
    assert_eq!(body["response"]["xff"], "10.0.0.99, 8.8.8.8, ::1");
    // S4 fix: origin is NOT the spoofed XFF; it reflects the TCP
    // peer (loopback, since the test connects locally).
    let origin = body["response"]["origin"].as_str().unwrap_or("");
    assert_ne!(
        origin, "10.0.0.99, 8.8.8.8, ::1",
        "S4 fix: XFF from an untrusted peer must not become origin"
    );
    assert!(
        origin == "127.0.0.1" || origin.starts_with("127.") || origin == "::1",
        "S4 fix: origin must be the socket peer (loopback), got {}",
        origin
    );
}

/// Complement to `xff_from_untrusted_peer_is_not_adopted_as_origin`:
/// when `proxy.trusted` includes the peer IP, XFF adoption resumes.
/// Pins the other half of the S4 contract so a regression that
/// silently DROPS the trust list (breaking real reverse-proxied
/// deployments) is caught.
#[tokio::test]
async fn xff_from_trusted_peer_is_adopted_as_origin() {
    let mut cfg = AppConfig::default();
    cfg.proxy = ruuter_on_rust::config::ProxyConfig {
        trusted: vec!["127.0.0.1".to_string()],
    };
    let router = build_router(
        cfg,
        &[(
            "svc/GET/probe.yml",
            r#"
reply:
  return:
    origin: "${incoming.origin}"
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/svc/probe", port))
        .header("x-forwarded-for", "10.0.0.99, 8.8.8.8, ::1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Post-N2 fix: the leftmost XFF value that parses as an IP wins
    // (RFC 7239 semantics). The trailing intermediate hops are
    // dropped from the framework-level `origin`.
    assert_eq!(
        body["response"]["origin"], "10.0.0.99",
        "trusted peer's XFF leftmost IP must be promoted into incoming.origin (whole chain no longer adopted verbatim)"
    );
}

// ── WS server abuse ────────────────────────────────────────────────

/// The WS server must gracefully accept binary frames that are NOT
/// valid UTF-8: the frame is silently ignored (per `run_ws_connection`),
/// not fed to `parse_payload`.
#[tokio::test]
async fn ws_binary_non_utf8_frame_does_not_crash() {
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/WS/echo.yml",
            r#"
reply:
  ws_send:
    payload: { ok: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let url = format!("ws://127.0.0.1:{}/svc/echo", port);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    use futures::SinkExt;
    ws.send(WsMsg::Binary(vec![0xff, 0xfe, 0xfd]))
        .await
        .unwrap();
    // Send a valid JSON text frame afterwards — the server must
    // still be alive to handle it.
    ws.send(WsMsg::Text(r#"{"a":1}"#.to_string()))
        .await
        .unwrap();
    use futures::StreamExt;
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await;
    assert!(
        first.is_ok(),
        "server hung after receiving a non-UTF-8 binary frame"
    );
}

// ── OpenAPI exposure sanity ────────────────────────────────────────

/// v0.9.11 (h2ck.me M1): flipped from "unauthenticated by design"
/// to "admin-gated by default." The pre-fix rationale ("describes
/// the same DSL any client can already probe") did not hold once
/// `declaration:` blocks with typed schemas landed — the spec now
/// surfaces declared PII field names, expected request shapes, and
/// response schemas that an internal admin API has no reason to
/// leak to unauthenticated callers. Contract:
/// - Public router (default): `/_/openapi.json` does NOT return the
///   spec (falls through to the DSL dispatch fallback).
/// - Admin router (mounted when `RUUTER_ADMIN_ENABLED=true`):
///   `/_/openapi.json` returns the spec.
#[tokio::test]
async fn openapi_spec_admin_gated_by_default() {
    use tower::ServiceExt;
    let router = std::sync::Arc::new(build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    ));
    let public_resp = router
        .clone()
        .build_axum_router_from_arc()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/_/openapi.json")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        public_resp.status().as_u16(),
        200,
        "public router must not enumerate the DSL surface via /_/openapi.json"
    );
    let admin_resp = router
        .admin_router()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/_/openapi.json")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_resp.status().as_u16(), 200);
    let bytes = axum::body::to_bytes(admin_resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.get("openapi").is_some() || body.get("info").is_some());
}

// ═════════════════════════════════════════════════════════════════════
// Gap-closing tests — added after the first OWASP coverage sweep.
// See projects/Ruuter-on-Rust/REVIEW.md § "Coverage matrix" for how
// these plug into the broader OWASP scorecard.
// ═════════════════════════════════════════════════════════════════════

// ── SSRF: redirect follow ──────────────────────────────────────────

/// **Finding S6** — `check_ssrf` runs ONCE before the reqwest request
/// is sent. reqwest by default follows redirects (up to 10). A DSL
/// call to an allowlisted URL that redirects to a blocked URL passes
/// the SSRF check, because the check does not re-run on the follow.
///
/// Attack scenario: operator whitelists `http://safe.example.com/`.
/// Attacker with control over `safe.example.com` (or that host's DNS)
/// makes it 302 to `http://169.254.169.254/latest/meta-data/` (cloud
/// metadata). The DSL call slips past.
///
/// This test spins up a redirect-server on 127.0.0.1 (whitelisted)
/// that 302s to another 127.0.0.1 port that is NOT whitelisted.
/// If the redirected request succeeds, S6 is active.
#[tokio::test]
async fn ssrf_check_does_not_reapply_after_redirect() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Blocked upstream — should never be reached if SSRF is
    // re-applied on the redirect target.
    let blocked = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blocked_port = blocked.local_addr().unwrap().port();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel::<()>();
    let hit_tx_clone = hit_tx.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = blocked.accept().await {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                        Content-Length: 15\r\n\r\n{\"secret\":true}",
                    )
                    .await;
                let _ = hit_tx_clone.send(());
            }
        }
    });

    // Whitelisted redirect server — 302 to the blocked port.
    let redir = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redir_port = redir.local_addr().unwrap().port();
    let redirect_target = format!("http://127.0.0.1:{}/", blocked_port);
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = redir.accept().await {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {}\r\n\
                    Content-Length: 0\r\nConnection: close\r\n\r\n",
                    redirect_target
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        }
    });

    // Allowlist ONLY the redirect origin — blocked origin is not on it.
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec![format!("http://127.0.0.1:{}/", redir_port)],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/start"
  result: r
  next: reply
reply:
  return: {{ status: "${{r.response.status}}", body: "${{JSON.stringify(r.response.body)}}" }}
  next: end
"#,
        redir_port
    );
    let router = build_router(cfg, &[("svc/GET/hop.yml", dsl.as_str())]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/hop", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();

    // Give the tokio task a moment to send on the channel.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let reached = hit_rx.try_recv().is_ok();

    // **S6 fixed** — the reqwest client is built with
    // `redirect(Policy::none())`, so the framework never follows the
    // 302. The DSL sees `status: 302` + `headers.location` and can
    // choose to re-fetch through `http.get`, which re-runs the SSRF
    // check on the new target. The blocked upstream must NOT be
    // contacted.
    assert!(
        !reached,
        "S6 regression: reqwest followed the 302 to a blocked origin — \
         redirect policy is not set to `Policy::none()` on the outbound client (body: {})",
        body
    );
    drop(hit_tx);
}

// ── SSRF: UDS + disabled interaction ───────────────────────────────

/// Companion to S3 — PINNED FIX. `internal_requests.disabled = true`
/// must block `unix://` scheme URLs too, matching the operator's
/// mental model of "no outbound HTTP, period." The S3 fix runs the
/// disabled-check at the top of `HttpClient::request`, before the
/// UDS dispatch, so this now returns the same
/// "outbound HTTP is disabled" error as the TCP path.
#[tokio::test]
async fn ssrf_unix_scheme_bypasses_outbound_disabled() {
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
            "svc/GET/uds.yml",
            r#"
fetch:
  call: http.get
  args:
    url: "unix:///nonexistent-socket.sock/foo"
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
        .get(format!("http://127.0.0.1:{}/svc/uds", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");

    // S3-ext PINNED FIX: the disabled guard runs before the UDS
    // dispatch, so a `unix://` URL is rejected the same way a TCP
    // URL is. The framework never opens the socket.
    assert!(
        err.contains("outbound HTTP is disabled"),
        "S3-ext fix: unix:// must be blocked when internal_requests.disabled=true — err: {}",
        err
    );
}

// ── Guards: HTTP-wire behaviour ────────────────────────────────────

/// Regression pin: a guard returning 401 short-circuits the main
/// DSL. The response body must come from the guard, not the main
/// DSL. Exercises the wire path (not just execute_dsl directly, as
/// tests/guards.rs does).
#[tokio::test]
async fn guard_401_short_circuits_before_main_dsl() {
    // If the main DSL runs despite the guard denying, its state.set
    // side-effect would be observable via a follow-up GET.
    let router = build_router(
        AppConfig::default(),
        &[
            (
                "svc/GET/protected.guard.yml",
                r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: allow
allow: { return: { ok: true }, next: end }
deny: { status: 401, return: { error: "no token" }, next: end }
"#,
            ),
            (
                "svc/GET/protected/data.yml",
                r#"
mark:
  state: { set: { key: "guard-bypass-marker", value: "leaked" } }
  next: reply
reply: { return: { data: "sensitive" }, next: end }
"#,
            ),
            (
                "svc/GET/probe-state.yml",
                r#"
read:
  state: { get: { key: "guard-bypass-marker", into: "v" } }
  next: reply
reply: { return: { seen: "${v}" }, next: end }
"#,
            ),
        ],
    );
    let port = serve(router).await;
    // Attack: hit protected/data without the token. Guard must
    // return 401 and the main DSL must NOT run its state.set.
    let denied = client()
        .get(format!("http://127.0.0.1:{}/svc/protected/data", port))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let body: serde_json::Value = denied.json().await.unwrap();
    assert_eq!(body["response"]["error"], "no token");
    assert!(
        body["response"].get("data").is_none(),
        "main DSL leaked through the guard: {}",
        body
    );

    // Follow-up: the state.set from the main DSL must not have run.
    let probe: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/svc/probe-state", port))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        probe["response"]["seen"].is_null() || probe["response"]["seen"] == serde_json::Value::Null,
        "the guarded DSL wrote state despite the guard denying: {}",
        probe
    );
}

/// Regression pin: `declaration.override_ancestors: true` on a
/// nested guard drops all ancestor guards for the routes it protects.
/// Assert this over the wire — a hostile inner override that lets
/// every caller through would silently disable an operator's outer
/// auth check. This test is a *defensibility pin*: the behaviour is
/// intended, but it should never be silent — the operator MUST know.
#[tokio::test]
async fn guard_override_ancestors_drops_outer_guards_over_wire() {
    let router = build_router(
        AppConfig::default(),
        &[
            (
                "svc/GET/api.guard.yml",
                r#"
check:
  switch:
    - condition: "${!incoming.headers['x-token']}"
      next: deny
  next: allow
allow: { return: { outer: "passed" }, next: end }
deny: { status: 401, return: { error: "outer says no" }, next: end }
"#,
            ),
            (
                "svc/GET/api/inject.guard.yml",
                r#"
declaration:
  override_ancestors: true
allow: { return: { inner_override: true }, next: end }
"#,
            ),
            (
                "svc/GET/api/inject/x.yml",
                r#"
ok: { return: { path: fault, ran: true }, next: end }
"#,
            ),
            (
                "svc/GET/api/normal/y.yml",
                r#"
ok: { return: { path: normal, ran: true }, next: end }
"#,
            ),
        ],
    );
    let port = serve(router).await;

    // Inject path: WITHOUT the outer token, still succeeds — proving
    // the override neutralised the outer guard.
    let bypass = client()
        .get(format!("http://127.0.0.1:{}/svc/api/inject/x", port))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bypass.status(),
        200,
        "override guard should let request through without outer token"
    );
    let body: serde_json::Value = bypass.json().await.unwrap();
    assert_eq!(body["response"]["ran"], true);

    // Normal path: outer guard STILL runs — 401 without token.
    let denied = client()
        .get(format!("http://127.0.0.1:{}/svc/api/normal/y", port))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        401,
        "override should only affect its own subtree, not siblings"
    );
}

// ── Error-detail leakage ───────────────────────────────────────────

/// A DSL that errors (unknown step, script error, upstream unreachable)
/// must not surface internal file paths, stack traces, or the
/// framework's own module names in the response body.
///
/// The current implementation surfaces `e.to_string()` verbatim in
/// the response `error` field. This test enumerates what leaks and
/// gives a hardened baseline to compare against once error responses
/// are sanitised.
#[tokio::test]
async fn error_bodies_do_not_leak_internal_paths() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/broken.yml",
            r#"
fetch:
  call: http.get
  args:
    url: "http://blackhole.invalid.tld:1/broken"
  result: r
  next: reply
reply:
  return: { ok: true }
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/broken", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("").to_string();

    // The Rust source path structure must never leak.
    for forbidden in [
        "/home/",
        "src/",
        "target/",
        ".rs:",
        "src/http_client/",
        "src/router/",
        "backtrace",
        "panicked",
    ] {
        assert!(
            !err.contains(forbidden),
            "error body leaks internal detail `{}`: {}",
            forbidden,
            err
        );
    }
}

/// The health endpoint must not surface internal detail on error.
/// Even a happy-path GET /health leaks the framework version — that
/// is S7.
#[tokio::test]
async fn health_endpoint_leaks_framework_version() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // **S7 fixed** — /health surfaces only `status: ok`. Framework
    // name + version no longer leak to unauthenticated probes.
    assert_eq!(
        body,
        serde_json::json!({ "status": "ok" }),
        "S7 regression: /health leaked more than {{ \"status\": \"ok\" }} — got {}",
        body
    );
}

// ── Rate limiting: pin the absence ─────────────────────────────────

/// Ruuter has no built-in rate limiter today. This test bursts 500
/// requests at a route in parallel and confirms none get rate-limited
/// — that pins the current design position. If a limiter ever lands,
/// this test must be replaced with one that verifies the limiter's
/// contract.
#[tokio::test]
async fn no_builtin_rate_limiter_500_concurrent_requests_all_2xx() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/GET/ping.yml",
            "reply:\n  return: { ok: true }\n  next: end\n",
        )],
    );
    let port = serve(router).await;
    let c = client();
    let mut handles = Vec::new();
    for _ in 0..500 {
        let c = c.clone();
        let url = format!("http://127.0.0.1:{}/svc/ping", port);
        handles.push(tokio::spawn(async move {
            c.get(url).send().await.map(|r| r.status().as_u16())
        }));
    }
    let mut ok = 0;
    let mut throttled = 0;
    for h in handles {
        match h.await.unwrap() {
            Ok(status) if (200..300).contains(&status) => ok += 1,
            Ok(status) if status == 429 || status == 503 => throttled += 1,
            _ => {}
        }
    }
    assert!(
        ok >= 490,
        "expected effectively no throttling, got ok={} throttled={}",
        ok,
        throttled
    );
    // If throttled ever spikes above 0 in this test, a rate limiter
    // has landed — expected to be a follow-up feature; replace this
    // test then.
    assert_eq!(
        throttled, 0,
        "unexpected 429/503 responses — rate limiter landed without replacing this pin?"
    );
}

// ── DSL loader: YAML anchor bomb ───────────────────────────────────

/// A YAML file with quadratic anchor expansion (`billion laughs`)
/// must fail to load rather than exhaust memory. Boot-time attack —
/// only exploitable by whoever writes the DSL file to disk, but the
/// loader still needs to be defensive.
///
/// serde_yml 0.0.12 depths its parse; we send a modest bomb (~13
/// levels, exponential factor 2) that would explode to 8k copies
/// only when materialised. Cap the wall-clock at 5s — a bomb that
/// hangs the loader for more is the risk.
#[tokio::test]
async fn dsl_yaml_anchor_bomb_does_not_hang_loader() {
    let bomb = r#"
a: &a "x"
b: &b [*a, *a, *a, *a, *a, *a, *a, *a]
c: &c [*b, *b, *b, *b, *b, *b, *b, *b]
d: &d [*c, *c, *c, *c, *c, *c, *c, *c]
e: &e [*d, *d, *d, *d, *d, *d, *d, *d]
respond:
  return: { note: "innocent-looking file" }
  next: end
"#;
    let tmp = std::env::temp_dir().join(format!("ruuter-yaml-bomb-{}", uuid()));
    let dsl_path = tmp.join("svc/GET/bomb.yml");
    std::fs::create_dir_all(dsl_path.parent().unwrap()).unwrap();
    std::fs::write(&dsl_path, bomb).unwrap();
    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;

    let start = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let loader = DslLoader::new(cfg.clone(), HashMap::new());
        loader.load_everything()
    }));
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "YAML anchor bomb hung the loader for {:?}",
        elapsed
    );
    // Regardless of Ok(loaded)/Err(parse err)/Err(panic), the important
    // thing is that we returned in a bounded time and didn't OOM.
    match result {
        Ok(Ok(_)) => { /* loader tolerated the bomb */ }
        Ok(Err(_)) => { /* loader rejected the bomb — fine */ }
        Err(_) => panic!("loader panicked on YAML anchor bomb"),
    }
}

// ── CVE floor: dependency advisory pin ─────────────────────────────

/// **Finding S8** — the current `Cargo.lock` contains crates flagged
/// by RustSec:
///
/// - RUSTSEC-2025-0003 (fast-float 0.2.0, no fix upstream): SIGSEGV
///   via missing bound check. Pulled in by `boa_engine 0.19.1`.
/// - RUSTSEC-2025-0068 (serde_yml 0.0.12): unsound + unmaintained.
///   Direct dep — the whole DSL / config parser sits on top of it.
/// - RUSTSEC-2025-0067 (libyml 0.0.5): unsound + unmaintained.
///   Transitive of serde_yml.
///
/// This test does NOT verify the CVEs are fixed (they aren't).
/// Instead it *pins* the presence of the known-bad versions so that a
/// deliberate downgrade (or an accidental re-pin during a `cargo
/// update`) is loud, and so that when the CVEs are addressed the test
/// changes accordingly.
///
/// When you fix the CVE:
/// - Remove the version from `KNOWN_BAD` below.
/// - Update projects/Ruuter-on-Rust/REVIEW.md § S8 accordingly.
#[test]
fn cve_floor_cargo_lock_contains_known_bad_versions() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"))
        .expect("read Cargo.lock");
    // **S8 fixed** — the crates that carried open RustSec advisories
    // at the pre-v1.0 audit are no longer in the lockfile. Each entry
    // below is a crate that MUST NOT reappear; if any regression pulls
    // it back in, the test fails and points at the review.
    let must_not_reappear: &[(&str, &str, &str)] = &[
        (
            "fast-float",
            "0.2.0",
            "RUSTSEC-2025-0003 / RUSTSEC-2024-0379",
        ),
        ("serde_yml", "0.0.12", "RUSTSEC-2025-0068"),
        ("libyml", "0.0.5", "RUSTSEC-2025-0067"),
    ];
    for (name, ver, id) in must_not_reappear {
        let needle = format!("name = \"{}\"\nversion = \"{}\"", name, ver);
        assert!(
            !lock.contains(&needle),
            "S8 regression: {}@{} reappeared in Cargo.lock (advisory {}) — \
             a dep bump pulled the vulnerable crate back in",
            name,
            ver,
            id
        );
    }
}

// ── Miscellaneous hardening pins ───────────────────────────────────

/// Audit finding 11 fix (post-2026-08-04): text/<subtype> bodies
/// are now parsed as `{ <subtype>: <body> }` (Java parity via
/// DslController.queryDslText). Verify the plain-text case reaches
/// the DSL as `incoming.body.plain = "hello world"`.
#[tokio::test]
async fn text_plain_body_is_wrapped_under_subtype_key() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/echo.yml",
            r#"
reply:
  return: { plain: "${incoming.body.plain}" }
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    let body: serde_json::Value = client()
        .post(format!("http://127.0.0.1:{}/svc/echo", port))
        .header("content-type", "text/plain")
        .body("hello world")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["response"]["plain"], "hello world");
}

/// The router allows `Content-Type: application/json; charset=utf-8`
/// and `Content-Type: application/foo+json`. This test pins the
/// permissive match — accepting `+json` MIME types is intentional
/// for JSON-LD, JSON Patch, etc.
#[tokio::test]
async fn plus_json_media_type_is_treated_as_json() {
    let router = build_router(
        AppConfig::default(),
        &[(
            "svc/POST/parse.yml",
            r#"
reply:
  return: { got: "${incoming.body.value}" }
  next: end
"#,
        )],
    );
    let port = serve(router).await;
    for ct in [
        "application/json",
        "application/json; charset=utf-8",
        "application/vnd.example+json",
    ] {
        let body: serde_json::Value = client()
            .post(format!("http://127.0.0.1:{}/svc/parse", port))
            .header("content-type", ct)
            .body(r#"{"value":"present"}"#)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            body["response"]["got"], "present",
            "Content-Type `{}` was not parsed as JSON",
            ct
        );
    }
}

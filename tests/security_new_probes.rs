//! Post-fix attack probes — h2ck.me follow-up sweep.
//!
//! The v1.0 fix batch closed S1, S2, S3, S3-ext, S4, S5. This file
//! attacks the seams and edges those fixes did not address, plus
//! pins the residual open findings (S6, S7, S8).
//!
//! Every test here follows the CLAUDE.md "break the fix" discipline:
//! we generate inputs that were NOT in the fix author's test set.

// Test-fixture AppConfig assembly. See tests/trigger_dispatch.rs for
// rationale.
#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::{AppConfig, InternalRequestsConfig, ProxyConfig};
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
    let tmp = std::env::temp_dir().join(format!("ruuter-newprobe-{}", uuid()));
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

// ── S2 residual: path-scoped entries still use starts_with ─────────
//
// The S2 fix made the ORIGIN comparison exact but kept
// `str::starts_with` for path-scoped entries. An operator writing
// `http://host/v1` (path segment, no trailing slash) still matches a
// substring-confusion request to `http://host/v1anything`.
//
// This is the same class of bug the S2 fix was supposed to close,
// just relocated from the authority-part to the path-part. A defender
// who intends "only paths under /v1/" must write `/v1/` (trailing
// slash) — the framework doesn't enforce that boundary.
//
// PoC: whitelist `http://127.0.0.1:<victim>/v1`, then call
// `http://127.0.0.1:<victim>/v1anything`. If the victim listener
// gets hit, the residual bypass is live.
#[tokio::test]
async fn s2_residual_path_scoped_entry_starts_with_bypasses_boundary() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let victim = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let victim_port = victim.local_addr().unwrap().port();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel::<String>();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = victim.accept().await {
                let mut buf = [0u8; 512];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request_line = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = hit_tx.send(request_line);
            }
        }
    });

    // Operator whitelist: only requests under /v1 should be allowed.
    // But the entry lacks a trailing slash, so `starts_with` opens the
    // door to /v1lookup, /v1beta, /v1private, ...
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec![format!("http://127.0.0.1:{}/v1", victim_port)],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/v1private/steal"
  result: r
  next: reply
reply:
  return: {{ status: "${{r.response.status}}" }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/leak.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/leak", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let hit = hit_rx.try_recv().ok();

    // **N1 fixed** — `allow_entry_matches` now requires the char
    // after the entry to be `/`, `?`, `#`, or end-of-string. A path
    // entry of `/v1` no longer admits `/v1private`. The victim must
    // NOT be contacted.
    assert!(
        hit.is_none(),
        "N1 regression: path-scoped entry `/v1` admitted `/v1private/steal` — \
         got hit `{}`",
        hit.unwrap_or_default()
    );
}

// ── S4 residual: whole-XFF-chain adoption ─────────────────────────
//
// Even when the peer is trusted, the framework adopts the ENTIRE
// X-Forwarded-For header value verbatim as `incoming.origin`. RFC 7239
// / Forwarded semantics: the leftmost value is the ORIGINAL client;
// any downstream intermediate hops are appended. Adopting the whole
// chain means a DSL that reads `origin` for audit or rate-limiting
// gets a comma-separated string that includes attacker-supplied noise
// from the leftmost position.
//
// PoC: send XFF `attacker.example, 10.0.0.1` from a trusted peer;
// confirm `origin` becomes the whole chain, not just `attacker.example`.
// A future defensive change would take-leftmost and reject the header
// entirely when it doesn't parse as an IP list.
#[tokio::test]
async fn s4_residual_xff_chain_adopted_whole_string_not_leftmost() {
    let mut cfg = AppConfig::default();
    // Trust localhost so XFF adoption fires.
    cfg.proxy = ProxyConfig {
        trusted: vec!["127.0.0.1".to_string()],
    };
    let dsl = r#"
respond:
  return: { origin_seen: "${incoming.origin}" }
  next: end
"#;
    let router = build_router(cfg, &[("svc/GET/who.yml", dsl)]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/who", port))
        .header("X-Forwarded-For", "attacker.example, 10.0.0.1")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let seen = body["response"]["origin_seen"].as_str().unwrap_or("");
    // **N2 fixed** — `resolve_origin` splits XFF on `,`, trims, and
    // takes the leftmost value only when it parses as an `IpAddr`.
    // A non-IP leftmost value (`attacker.example`) fails the parse
    // and the framework falls back to the socket peer. Downstream
    // DSLs never see the attacker-controlled string.
    assert_eq!(
        seen, "127.0.0.1",
        "N2 regression: XFF leftmost accepted a non-IP value; got `{}`",
        seen
    );
}

// ── S4 IPv6 dual-stack trust footgun ──────────────────────────────
//
// Operator config `proxy.trusted: ["127.0.0.1"]` looks correct, but
// axum reports the peer IP as reported by `SocketAddr::ip()` — which
// on a dual-stack IPv6 socket appears as `::ffff:127.0.0.1`. String
// comparison fails, XFF is dropped, and audit logs show the peer IP
// instead of the client IP. Not a security bug (safe default), but
// a footgun the docs should warn about.
//
// This test pins the current string-comparison semantics.
#[tokio::test]
async fn s4_ipv6_mapped_ipv4_string_mismatch_drops_trust() {
    let mut cfg = AppConfig::default();
    cfg.proxy = ProxyConfig {
        // v4-mapped-v6 form; a v4-only peer will NOT match this.
        trusted: vec!["::ffff:127.0.0.1".to_string()],
    };
    let dsl = r#"
respond:
  return: { origin_seen: "${incoming.origin}" }
  next: end
"#;
    let router = build_router(cfg, &[("svc/GET/who.yml", dsl)]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/who", port))
        .header("X-Forwarded-For", "192.0.2.99")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let seen = body["response"]["origin_seen"].as_str().unwrap_or("");
    // **N3 fixed** — `resolve_origin` now parses both the peer IP
    // and each `trusted` entry as `IpAddr` and canonicalises
    // IPv4-mapped IPv6 back to plain IPv4. An operator writing
    // `::ffff:127.0.0.1` and a peer arriving as `127.0.0.1` compare
    // equal, so XFF adoption fires and the client IP surfaces.
    assert_eq!(
        seen, "192.0.2.99",
        "N3 regression: canonicalisation did not match IPv4-mapped IPv6 against IPv4 peer; got `{}`",
        seen
    );
}

// ── Cloud metadata: unfixed by design ─────────────────────────────
//
// With the DEFAULT config (empty `allowed_urls`, empty `allowed_ips`),
// there is no restriction on outbound targets. A DSL can call
// `http://169.254.169.254/...` — the AWS/GCP/Azure metadata endpoint.
// This is not a fix regression; it is the framework's default posture.
// Operators MUST set `internal_requests.allowed_ips` or `allowed_urls`
// to close this. Pin the default behaviour so any future change
// (e.g. blocklist of link-local + RFC1918 by default) is intentional.
#[tokio::test]
async fn default_config_permits_link_local_metadata_target() {
    // We don't try to actually reach 169.254 (won't be routable in
    // most test envs). Instead we set up a listener on 127.0.0.1 and
    // pin that with an empty allowlist, the outbound is issued (i.e.
    // the SSRF check does not block link-local-like or private-IP
    // requests by default).
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let victim = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let victim_port = victim.local_addr().unwrap().port();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel::<()>();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = victim.accept().await {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\ncreds")
                    .await;
                let _ = hit_tx.send(());
            }
        }
    });

    // Default config — no allowlist, no disable, and (post-N4-fix)
    // `block_private_networks: true`.
    let cfg = AppConfig::default();
    let dsl = format!(
        r#"
grab:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/latest/meta-data/iam/security-credentials/role"
  result: r
  next: reply
reply:
  return: {{ leaked: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/metadata.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/metadata", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // **N4 fixed** — the default `block_private_networks: true`
    // rejects outbound TCP to loopback / link-local / RFC-1918 /
    // ULA. The victim listener must NOT be contacted. Operators who
    // legitimately need a private-network sidecar over TCP loopback
    // opt back in by setting `block_private_networks: false` or
    // listing the host under `allowed_ips` / `allowed_urls`.
    assert!(
        hit_rx.try_recv().is_err(),
        "N4 regression: default config allowed outbound to 127.0.0.1 — \
         `block_private_networks: true` did not fire"
    );
}

// ── S3 completeness: check_ssrf allowlist still gated by disabled ─
//
// The S3 fix moved the `outbound_disabled` check to the top of
// `HttpClient::request`. Verify that check_ssrf's own vestigial
// `outbound_disabled` branch (still present at line ~245) is dead
// code — the top-level gate always fires first. Attack: turn on
// `disabled` AND set an allowlist that would otherwise match. The
// error message must reference "disabled", not the allowlist.
#[tokio::test]
async fn s3_completeness_disabled_takes_precedence_over_allowlist() {
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: true,
        allowed_ips: vec![],
        allowed_urls: vec!["http://example.invalid".to_string()],
        block_private_networks: false,
    };
    let dsl = r#"
fetch:
  call: http.get
  args:
    url: "http://example.invalid/anything"
  result: r
  next: reply
reply:
  return: { done: true }
  next: end
"#;
    let router = build_router(cfg, &[("svc/GET/gated.yml", dsl)]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/gated", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("disabled"),
        "S3 completeness: disabled must fire before allowlist. Got: {}",
        err
    );
}

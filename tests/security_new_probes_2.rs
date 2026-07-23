//! Second-round "break the fix" probes — h2ck.me post-052-060 sweep.
//!
//! The v1.x fix batch closed S6, S7, S8, and residuals N1-N4. This
//! file attacks the seams of those NEW fixes, focussing on inputs
//! the fix author almost certainly didn't test. CLAUDE.md rule 3:
//! "tests written to confirm a fix only catch bugs you already knew
//! about" — so every test here starts from "what wrong input would
//! slip past this?"

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
    let tmp = std::env::temp_dir().join(format!("ruuter-p2-{}", uuid()));
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

/// **BREAK-N1** — the N1 fix requires the char after `entry_full` to
/// be `/`, `?`, `#`, or end-of-string. But `entry_full` includes any
/// trailing slash the operator wrote. Trace:
///   entry = `http://host/v1/`  →  entry_full = `...:80/v1/`
///   req   = `http://host/v1/foo` → req_full = `...:80/v1/foo`
///   tail  = `foo`  →  none of `/?#`, non-empty  →  REJECT.
///
/// If that trace is right, the fix breaks the LEGITIMATE prefix
/// convention: an operator writing `http://host/v1/` intending
/// "everything under /v1/" now gets nothing under /v1/ accepted.
/// This is a functional regression that would surface the first time
/// a defender does the recommended thing (add a trailing slash).
#[tokio::test]
async fn break_n1_trailing_slash_entry_rejects_legitimate_subpath() {
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
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = hit_tx.send(());
            }
        }
    });
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        // Operator writes trailing slash — this is the recommended
        // "close the boundary" style.
        allowed_urls: vec![format!("http://127.0.0.1:{}/v1/", victim_port)],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/v1/legit"
  result: r
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/legit.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/legit", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let reached = hit_rx.try_recv().is_ok();
    // F1 fixed — an entry that already ends at a URL delimiter (`/`,
    // `?`, `#`) closes the boundary itself; the tail check is not
    // applied in that case. `.../v1/` therefore admits `.../v1/legit`.
    assert!(
        reached,
        "F1 regression: entry `.../v1/` must admit request `.../v1/legit` \
         (the entry's trailing `/` closes the segment boundary; anything \
         after it lives in the next segment)."
    );
}

/// **F1 positive** — exact-match of a trailing-slash entry. The N1
/// fix must not reject the entry's own path (`/v1/` should match
/// `/v1/` verbatim, without extra segments).
#[tokio::test]
async fn f1_positive_trailing_slash_entry_admits_exact_path() {
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
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = hit_tx.send(());
            }
        }
    });
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec![format!("http://127.0.0.1:{}/v1/", victim_port)],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/v1/"
  result: r
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/exact.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/exact", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        hit_rx.try_recv().is_ok(),
        "F1 positive: entry `.../v1/` must admit exact request `.../v1/`"
    );
}

/// **F1 positive (query-scoped entry)** — `?` closes the boundary
/// the same way `/` does. An entry with a query pinned by the
/// operator must admit itself, and must admit the same URL with
/// additional query params appended (`&extra=1`).
#[tokio::test]
async fn f1_positive_query_scoped_entry_admits_same_and_extended_query() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let victim = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let victim_port = victim.local_addr().unwrap().port();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel::<u32>();
    tokio::spawn(async move {
        let mut hit_ct: u32 = 0;
        loop {
            if let Ok((mut stream, _)) = victim.accept().await {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                hit_ct += 1;
                let _ = hit_tx.send(hit_ct);
            }
        }
    });
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec![format!("http://127.0.0.1:{}/v1?tok=X", victim_port)],
        block_private_networks: false,
    };
    // Two calls: same query, and query with extra param appended.
    // The DSL fans out to both via `next:` chaining.
    let dsl = format!(
        r#"
one:
  call: http.get
  args:
    url: "http://127.0.0.1:{p}/v1?tok=X"
  result: a
  next: two
two:
  call: http.get
  args:
    url: "http://127.0.0.1:{p}/v1?tok=X&extra=1"
  result: b
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        p = victim_port
    );
    let router = build_router(cfg, &[("svc/GET/qs.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/qs", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    // Drain the channel — we expect both requests to arrive.
    let mut last = 0u32;
    while let Ok(n) = hit_rx.try_recv() {
        last = n;
    }
    assert_eq!(
        last, 2,
        "F1 positive (query): entry `?tok=X` must admit both the exact URL \
         and the same URL with an appended `&extra=1`; got {} of 2 hits",
        last
    );
}

/// **BREAK-N4** — the private-network blocklist parses the URL host as
/// an `IpAddr`. When the host is a HOSTNAME (not a literal IP), the
/// blocklist skips it entirely. `localhost` resolves to 127.0.0.1;
/// `metadata.google.internal` resolves to 169.254.169.254; DNS
/// records the attacker controls can point anywhere.
///
/// This test uses `localhost` — which DNS reliably resolves to
/// 127.0.0.1 on any Linux test host — to prove the blocklist misses
/// hostname-encoded loopback. The victim listener on 127.0.0.1:<port>
/// receives the request even with `block_private_networks: true`
/// (the default).
#[tokio::test]
async fn break_n4_hostname_to_private_ip_bypasses_blocklist() {
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
    // Default config: block_private_networks defaults to true; no
    // allowlist. If the blocklist worked by resolved-IP, this would
    // reject the request. Because it works by URL-host-string, a
    // hostname bypasses.
    let cfg = AppConfig::default();
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://localhost:{}/latest/meta-data/iam/security-credentials/role"
  result: r
  next: reply
reply:
  return: {{ leaked: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/bypass.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/bypass", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let reached = hit_rx.try_recv().is_ok();
    // **F2 fixed** — `check_ssrf` now resolves hostnames via
    // `tokio::net::lookup_host` and applies `is_private_or_local` to
    // every returned address. `localhost` → `127.0.0.1` fails the
    // check and the request is rejected before reaching the victim.
    assert!(
        !reached,
        "F2 regression: `localhost` bypassed block_private_networks — \
         hostname → private-IP resolution not being checked in check_ssrf"
    );
}

/// **F2 positive** — an explicit allowlist entry opts the hostname
/// back in, even when `block_private_networks` is on. Any operator
/// with a legitimate loopback-sidecar integration configures the
/// allowlist explicitly; the blocklist branch skips itself when
/// either allowlist is non-empty.
#[tokio::test]
async fn f2_positive_allowlisted_hostname_passes_even_with_blocklist_on() {
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
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = hit_tx.send(());
            }
        }
    });
    let mut cfg = AppConfig::default();
    // block_private_networks stays true (default). The allowlist opts
    // this specific hostname back in.
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec![format!("http://localhost:{}/", victim_port)],
        block_private_networks: true,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://localhost:{}/sidecar"
  result: r
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/allow.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/allow", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(
        hit_rx.try_recv().is_ok(),
        "F2 positive: an explicit `allowed_urls` entry must bypass the \
         block_private_networks blocklist for that hostname"
    );
}

/// **BREAK-N4 variant** — decimal-encoded IPv4 also bypasses the
/// blocklist. `2130706433 == 127.0.0.1`. `url::Url::parse` accepts
/// this, `parsed.host_str()` returns `"2130706433"`, `.parse::<IpAddr>()`
/// FAILS (parse doesn't accept decimal), so the blocklist skips it.
/// reqwest / hyper's DNS resolver then converts the numeric host into
/// the IPv4 address on the wire.
///
/// If this test's victim is contacted, the blocklist is bypassed by
/// integer-encoded loopback.
#[tokio::test]
async fn break_n4_decimal_encoded_ipv4_bypasses_blocklist() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Compute the decimal representation of 127.0.0.1 for readability.
    // 127*2^24 + 0*2^16 + 0*2^8 + 1 = 2130706433.
    let victim = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let victim_port = victim.local_addr().unwrap().port();
    let (hit_tx, hit_rx) = std::sync::mpsc::channel::<()>();
    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = victim.accept().await {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = hit_tx.send(());
            }
        }
    });
    let cfg = AppConfig::default();
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://2130706433:{}/latest/meta-data/"
  result: r
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/dec.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/dec", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    // Test-condition: whether the victim was hit. We record the actual
    // observed outcome (may vary by url crate version) as the current
    // pin; the point is to document behaviour, not to fail if the
    // parser already covers this edge.
    let reached = hit_rx.try_recv().is_ok();
    if reached {
        panic!(
            "N4 bypass: decimal-encoded IPv4 (2130706433 == 127.0.0.1) \
             bypasses block_private_networks — URL-host-string check does \
             not parse integer-encoded hosts as IP."
        );
    }
    // If NOT reached, either the URL parser rejected it (safe) or
    // the DNS resolver failed to interpret it (also safe on this
    // stack). Documented pin: current behaviour is safe on this
    // build; keep the test so a future url-crate upgrade doesn't
    // silently regress.
}

/// **BREAK-N2** — the leftmost-IP-only fix falls back to X-Real-IP
/// only when XFF is absent OR the leftmost XFF chunk is empty. It
/// does NOT try X-Real-IP when XFF's leftmost value is present-but-
/// unparseable. So a legitimate proxy that passes
///   `X-Forwarded-For: hostname.example, 10.0.0.1`
///   `X-Real-IP: 203.0.113.5`
/// loses X-Real-IP: XFF leftmost is "hostname.example" (not an IP),
/// falls through — but the code then also skips X-Real-IP. `origin`
/// reflects the peer, not the correct client IP.
///
/// Not a security bug — safer default (falls back to peer). Documents
/// a minor logic quirk operators may hit with mixed-config proxies.
#[tokio::test]
async fn n2_xff_non_ip_leftmost_does_not_fall_back_to_xrealip() {
    let mut cfg = AppConfig::default();
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
        .header("X-Forwarded-For", "hostname.example, 10.0.0.1")
        .header("X-Real-IP", "203.0.113.5")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let seen = body["origin_seen"].as_str().unwrap_or("");
    // Documented behaviour: falls all the way through to the peer,
    // ignoring both a non-IP XFF leftmost AND the valid X-Real-IP.
    // If a future fix threads X-Real-IP as a real fallback, `seen`
    // becomes `203.0.113.5`.
    assert_eq!(
        seen, "127.0.0.1",
        "quirk: when XFF leftmost is present-but-not-an-IP, X-Real-IP \
         is NOT considered as a fallback; origin falls to the peer. If \
         `seen` is `203.0.113.5`, that quirk was fixed."
    );
}

/// **BREAK-N3** — the trusted-proxy list is String-typed in config.
/// The fix parses each entry with `.parse::<IpAddr>().ok()` and
/// silently drops entries that don't parse. An operator who typoed
/// `trusted: ["127.0.0.1/8"]` (thinking CIDR is supported) gets
/// ZERO trust — the entry is dropped without warning.
///
/// Failure mode is safe (no trust granted, XFF ignored, `origin`
/// reflects peer). But there's no boot-time log or config-validation
/// error to surface the typo. Documented pin.
#[tokio::test]
async fn n3_unparseable_trusted_entries_silently_dropped() {
    let mut cfg = AppConfig::default();
    cfg.proxy = ProxyConfig {
        trusted: vec![
            "127.0.0.1/8".to_string(),   // CIDR attempt — invalid IpAddr
            "not-an-ip".to_string(),     // garbage
            "192.168.1.999".to_string(), // out-of-range octet
        ],
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
        .header("X-Forwarded-For", "203.0.113.5")
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let seen = body["origin_seen"].as_str().unwrap_or("");
    // All three trusted entries are unparseable → trusted list is
    // effectively empty → peer is not trusted → XFF is ignored →
    // origin reflects the peer. No warning is emitted; the operator
    // has no signal that their config is a typo.
    assert_eq!(
        seen, "127.0.0.1",
        "unparseable trusted entries silently discarded — no boot-time \
         warning. This is safe but a footgun; consider validating at \
         load time and refusing to boot on any unparseable entry."
    );
}

/// **CONFIRM-S7** — /health returns exactly `{"status":"ok"}`. No
/// service name, no version.
#[tokio::test]
async fn confirm_s7_health_returns_only_status_ok() {
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
    assert_eq!(body, serde_json::json!({"status": "ok"}));
    assert!(
        body.get("service").is_none(),
        "S7 regression: `service` leaked"
    );
    assert!(
        body.get("version").is_none(),
        "S7 regression: `version` leaked"
    );
}

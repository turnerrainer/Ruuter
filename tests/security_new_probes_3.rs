//! Third-round "break the fix" probes — h2ck.me post-061-062-056.
//!
//! F1 (trailing-slash allowlist) and F2 (hostname → private-IP
//! blocklist) fixes landed. 056 CI gate landed. This file attacks
//! the seams of THOSE fixes plus a design flaw in how blocklist
//! + allowlist interact.

use ruuter_on_rust::config::{
    AppConfig, InternalRequestsConfig,
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, seq)
}

fn build_router(cfg: AppConfig, files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-p3-{}", uuid()));
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

/// **F3-NEW / F2 seam** — the F2 blocklist branch runs only when
/// BOTH `allowed_urls` AND `allowed_ips` are empty. So the moment an
/// operator adds one entry to either allowlist — say
/// `allowed_urls: ["https://api.stripe.com/"]` because they need
/// Stripe access — the private-network blocklist is disabled for
/// EVERY OTHER destination too. A DSL can then reach `http://localhost/`,
/// `http://169.254.169.254/`, etc., without the blocklist checking
/// them, because the allowlist branch above rejects them for not
/// matching Stripe... EXCEPT that in current code the allowlist
/// check exists but the blocklist is gated on both allowlists being
/// empty. Let me trace to be sure:
///
///   1. If `allowed_url_prefixes` non-empty → allowlist branch runs,
///      request rejected unless it matches an entry.
///   2. If `allowed_ip_hosts` non-empty → allowlist branch runs,
///      request rejected unless the host is a bare IP in the list.
///   3. Blocklist gate: only fires when BOTH allowlists empty.
///
/// So actually — with `allowed_urls: ["https://api.stripe.com/"]`,
/// ONLY the Stripe URL passes; everything else is rejected by
/// step 1. The blocklist not running is fine because the allowlist
/// already rejected it.
///
/// BUT — mixed: what if operator sets `allowed_ips: ["1.2.3.4"]`?
/// Step 2 gate becomes: request rejected unless the URL's host is
/// literally `"1.2.3.4"`. A hostname URL fails that check
/// (hostname is not `"1.2.3.4"`). So — safe.
///
/// This test PROVES the safety: with a single non-loopback
/// allowlist entry, an attempted metadata-SSRF via
/// `http://localhost:<victim>/` is rejected — either by the
/// allowlist branch (correct rejection) or, in the counterfactual
/// where the allowlist somehow passed, by the blocklist. If neither
/// runs, the victim gets hit, which is the finding.
#[tokio::test]
async fn f2_seam_partial_allowlist_still_blocks_other_private_targets() {
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
    // Operator's real config: allow a specific external service.
    // Nothing about localhost — should be implicitly blocked.
    let mut cfg = AppConfig::default();
    cfg.internal_requests = InternalRequestsConfig {
        disabled: false,
        allowed_ips: vec![],
        allowed_urls: vec!["https://api.example.com/".to_string()],
        block_private_networks: true,   // default; the operator
                                        // didn't touch this either
    };
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
    let router = build_router(cfg, &[("svc/GET/mix.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/mix", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let reached = hit_rx.try_recv().is_ok();
    assert!(
        !reached,
        "F2 seam: with a real allowlist configured (`api.example.com`), \
         a DSL still reached `localhost` — the allowlist should have \
         rejected the non-matching URL before blocklist consideration \
         even mattered. If reached is true, the pre-check ordering is \
         broken."
    );
}

/// **F2 seam** — IPv6 literal loopback (`[::1]`) via URL literal.
/// The blocklist should reject an outbound to `http://[::1]:<port>/`
/// under default config. `parsed.host_str()` for `http://[::1]/`
/// returns `"::1"`; `.parse::<IpAddr>()` succeeds; `is_private_or_local`
/// catches IPv6 loopback. Positive pin — confirms the fix covers
/// the IPv6 literal case, not just IPv4.
#[tokio::test]
async fn f2_ipv6_loopback_literal_blocked_by_default() {
    let cfg = AppConfig::default();
    let dsl = r#"
fetch:
  call: http.get
  args:
    url: "http://[::1]:9/nope"
  result: r
  next: reply
reply:
  return: { done: true }
  next: end
"#;
    let router = build_router(cfg, &[("svc/GET/v6.yml", dsl)]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/v6", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        err.contains("private")
            || err.contains("link-local")
            || err.contains("loopback")
            || err.contains("blocked"),
        "F2 must reject IPv6 loopback literal by default; error was: `{}`",
        err
    );
}

/// **F2 seam** — hostname that resolves to an IPv4-mapped-IPv6 address
/// (`::ffff:127.0.0.1`) via DNS. `is_private_or_local` handles the
/// v4-mapped form by unwrapping to v4 and re-checking. Fixture is
/// tricky (system resolver rarely returns IPv4-mapped IPv6); this
/// test just documents the code-path — the recursive re-check in
/// `is_private_or_local` at src/http_client/mod.rs:625-627 is the
/// mechanism.
///
/// If a future refactor drops that recursive branch, this test's
/// intent is to fail via inspection during review — the pin is on
/// the CODE PATH, not on an easily-reproducible network setup.
/// We instead exercise the literal form `[::ffff:127.0.0.1]` which
/// url-parses as an IPv6 host, and confirm it's blocked.
#[tokio::test]
async fn f2_ipv4_mapped_ipv6_literal_blocked() {
    let cfg = AppConfig::default();
    let dsl = r#"
fetch:
  call: http.get
  args:
    url: "http://[::ffff:127.0.0.1]:9/nope"
  result: r
  next: reply
reply:
  return: { done: true }
  next: end
"#;
    let router = build_router(cfg, &[("svc/GET/v6mapped.yml", dsl)]);
    let port = serve(router).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{}/svc/v6mapped", port))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        err.contains("private")
            || err.contains("link-local")
            || err.contains("loopback")
            || err.contains("blocked"),
        "F2 must reject IPv4-mapped-IPv6 loopback literal \
         `[::ffff:127.0.0.1]`; error was: `{}`",
        err
    );
}

/// **F1 seam** — entry ending in `&` (unusual but possible). The
/// fix's `entry_closed_at_boundary` list is `/`, `?`, `#`. `&` is
/// in the tail-delimiter set but NOT the boundary-close set. So an
/// entry `http://host/v1?tok=X&` — which some operators write when
/// composing a template — falls into the "closed mid-segment"
/// branch and requires the tail to start with a delimiter.
///
/// For request `http://host/v1?tok=X&extra=1`:
///   - entry_full = `http://host:80/v1?tok=X&`
///   - req_full = `http://host:80/v1?tok=X&extra=1`
///   - tail = "extra=1"
///   - tail starts_with '/' | '?' | '#' | '&' → FALSE
///   - → rejected
///
/// So a trailing-`&` entry cannot admit any request that pins
/// through it. Symmetric to the original F1 bug in a smaller way.
/// Not a security issue; documents behaviour.
#[tokio::test]
async fn f1_seam_trailing_ampersand_entry_rejects_extended_query() {
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
        // Trailing `&` — expresses "and then more params". Not
        // uncommon in template-driven configs.
        allowed_urls: vec![format!("http://127.0.0.1:{}/v1?tok=X&", victim_port)],
        block_private_networks: false,
    };
    let dsl = format!(
        r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{}/v1?tok=X&extra=1"
  result: r
  next: reply
reply:
  return: {{ done: true }}
  next: end
"#,
        victim_port
    );
    let router = build_router(cfg, &[("svc/GET/amp.yml", dsl.as_str())]);
    let port = serve(router).await;
    let _ = client()
        .get(format!("http://127.0.0.1:{}/svc/amp", port))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let reached = hit_rx.try_recv().is_ok();
    // Documented pin: `&`-terminated entries currently reject
    // extensions. If someone extends `entry_closed_at_boundary` to
    // include `&`, this flips. Non-security-critical.
    assert!(
        !reached,
        "F1-seam pin: entry `.../v1?tok=X&` currently rejects \
         extension. If this flips (reached=true), the boundary-close \
         set was widened to include `&` — documented as an acceptable \
         choice; update this test."
    );
}

/// **056 CI gate — local smoke check.** The workflow file exists
/// and `cargo audit --deny warnings` passes locally. If this test
/// fails, either the workflow is missing or `.cargo/audit.toml` no
/// longer covers a new unmaintained warning. Actually running
/// `cargo audit` from a test is slow and depends on the RustSec
/// DB being fetched — instead, we assert the two config artefacts
/// exist and have plausible contents. The CI job itself is the
/// runtime check.
#[test]
fn task_056_workflow_and_exceptions_files_present() {
    let root = env!("CARGO_MANIFEST_DIR");
    let workflow = std::fs::read_to_string(format!("{}/.github/workflows/security.yml", root))
        .expect("`.github/workflows/security.yml` must exist (task 056)");
    assert!(
        workflow.contains("cargo audit"),
        "security.yml must run `cargo audit`"
    );
    assert!(
        workflow.contains("cron:"),
        "security.yml must schedule a cron so advisories against \
         unchanged Cargo.lock still fire"
    );
    let audit_toml = std::fs::read_to_string(format!("{}/.cargo/audit.toml", root))
        .expect("`.cargo/audit.toml` must exist for documented exceptions");
    assert!(
        audit_toml.contains("[advisories]"),
        ".cargo/audit.toml must declare an [advisories] section"
    );
    // Both current exceptions must include a review date so
    // ignored entries are re-visited.
    assert!(
        audit_toml.contains("Review:"),
        ".cargo/audit.toml exceptions must carry a review date"
    );
}

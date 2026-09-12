//! h2ck.me v1 T-16 — unknown-project 404 no longer echoes the
//! client's URL path segment as the trace-context `project=` field.
//!
//! Pre-fix, `handle_request` set the span field
//! `dsl.project = %project_for_span` from the raw first URL segment.
//! An attacker probing `POST /candidate-name/foo` for every
//! candidate name saw their guess reflected in structured logs —
//! a mild project-name enumeration signal for operators who
//! consumed the logs.
//!
//! Post-fix, if the first path segment doesn't match a KNOWN
//! project (from `router.dsls`), the span/log field surfaces as
//! `<unknown>`. `http.route` still carries the full URL path so
//! debugging isn't impaired; `dsl.project` no longer carries
//! attacker-controlled bytes.
//!
//! Tests written to try to BREAK the fix:
//! - Request to a KNOWN project → span field is the real project.
//! - Request to an UNKNOWN project → span field is `<unknown>`
//!   AND does NOT contain the client's original segment.
//! - Access log line reflects the same substitution.
//! - `http.route` still contains the full URL path in both cases
//!   (so operators can still see what was tried).

#![allow(clippy::field_reassign_with_default)]

use axum::body::Body;
use axum::http::Request;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

fn build_router(files: &[(&str, &str)]) -> DslRouter {
    let mut cfg = AppConfig::default();
    // Force the access log on so we can inspect what's captured.
    cfg.logging.access_log = true;
    let tmp = std::env::temp_dir().join(format!("ruuter-T16-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
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

#[derive(Clone)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture(buf: SharedBuf) -> tracing::subscriber::DefaultGuard {
    use tracing_subscriber::{fmt, EnvFilter};
    let subscriber = fmt()
        .with_writer(buf)
        .with_max_level(tracing::Level::INFO)
        .with_env_filter(EnvFilter::new("info"))
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::set_default(subscriber)
}

const OK_RESPONSE: &str = r#"
respond:
  return: { ok: true }
  status: 200
  next: end
"#;

#[tokio::test]
async fn known_project_appears_verbatim_in_access_log() {
    let router = build_router(&[("svc/GET/ping.yml", OK_RESPONSE)]);
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let app = router.build_axum_router();
    let _ = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/svc/ping")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    drop(_g);
    let out = buf.contents();
    // The access log names dsl.project. Known project → verbatim.
    assert!(
        out.contains("dsl.project=\"svc\"") || out.contains("dsl.project=svc"),
        "known project must appear verbatim in access log; got:\n{out}"
    );
}

#[tokio::test]
async fn unknown_project_appears_as_unknown_in_access_log() {
    let router = build_router(&[("svc/GET/ping.yml", OK_RESPONSE)]);
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let app = router.build_axum_router();
    let _ = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/candidate-name/foo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    drop(_g);
    let out = buf.contents();
    // Post-fix contract: dsl.project is <unknown>, NOT the
    // client's first segment.
    assert!(
        out.contains("<unknown>"),
        "unknown project must render as <unknown>; got:\n{out}"
    );
    assert!(
        !out.contains("dsl.project=\"candidate-name\"")
            && !out.contains("dsl.project=candidate-name"),
        "attacker-controlled first segment must NOT leak into dsl.project; got:\n{out}"
    );
}

#[tokio::test]
async fn unknown_project_full_route_still_in_http_route() {
    // Debugging invariant: `http.route` still contains the raw URL
    // path so operators can see what was probed, WITHOUT the raw
    // first segment leaking into the semantic `dsl.project` field.
    let router = build_router(&[("svc/GET/ping.yml", OK_RESPONSE)]);
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let app = router.build_axum_router();
    let _ = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/candidate-name/foo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    drop(_g);
    let out = buf.contents();
    // http.route field is the raw URL — kept intact for
    // debugging. The T-16 fix is scoped narrowly to dsl.project.
    assert!(
        out.contains("/candidate-name/foo"),
        "http.route must preserve the raw path for debugging; got:\n{out}"
    );
}

#[tokio::test]
async fn empty_path_defaults_to_unknown() {
    // `GET /` (no path segments at all) — the resolver's raw
    // first-segment falls back to `""`, which will never be a
    // registered project → surfaces as <unknown>.
    let router = build_router(&[("svc/GET/ping.yml", OK_RESPONSE)]);
    let buf = SharedBuf::new();
    let _g = capture(buf.clone());
    let app = router.build_axum_router();
    let _ = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("send");
    drop(_g);
    let out = buf.contents();
    // Either <unknown> appears OR the empty string was silently
    // used — pin the intended behaviour explicitly.
    assert!(
        out.contains("<unknown>"),
        "empty path first-segment must render as <unknown>, not empty; got:\n{out}"
    );
}

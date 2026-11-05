//! OTLP export: trace-context continuity, OTel span fields and exporter
//! configuration.
//!
//! Pre-fix, with `OTEL_EXPORTER_OTLP_ENDPOINT` set:
//! - the exported `http_request` span started a NEW trace, unrelated to the
//!   adopted inbound `traceparent`, so exported traces never matched `X-Trace-Id`,
//!   logs or downstream services;
//! - it had kind INTERNAL, no `http.response.status_code`, no error status;
//! - the resource carried only `service.name` (`OTEL_RESOURCE_ATTRIBUTES`
//!   ignored), and https:// or http/protobuf collectors were unreachable.
//!
//! Post-fix the request span is a SERVER span parented on the inbound
//! context, the propagated/echoed `traceparent` is the span's own context
//! (same trace id), and the exporter honours TLS, protocol, headers and the
//! resource env vars. With export OFF nothing changes; that path stays
//! covered by `security.rs::traceparent_adopted_from_request_and_echoed`.
#![allow(clippy::field_reassign_with_default)]
use axum::body::Body;
use axum::http::Request;
use futures::future::BoxFuture;
use opentelemetry::trace::{SpanKind, Status, TracerProvider as _};
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use opentelemetry_sdk::trace::TracerProvider;
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::observability;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Debug, Clone, Default)]
struct Captured(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Captured {
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        self.0.lock().unwrap().extend(batch);
        Box::pin(async { Ok(()) })
    }
}

impl Captured {
    fn request_spans(&self) -> Vec<SpanData> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.name.starts_with("HTTP "))
            .cloned()
            .collect()
    }
}

fn attr(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
}

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

fn build_router(files: &[(&str, &str)]) -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-otel-{}", uuid()));
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

/// Thread-local subscriber with an OTel layer exporting into `Captured`.
fn otel_capture() -> (Captured, TracerProvider, tracing::subscriber::DefaultGuard) {
    observability::set_otel_active_for_tests(true);
    let captured = Captured::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(captured.clone())
        .build();
    let tracer = provider.tracer("test");
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, provider, guard)
}

const OK: &str = "respond:\n  return:\n    ok: true\n  status: 200\n  next: end\n";
const FAIL: &str = "respond:\n  return:\n    ok: false\n  status: 503\n  next: end\n";

async fn send(router: DslRouter, uri: &str, traceparent: Option<&str>) -> (u16, String, String) {
    let mut req = Request::builder().method("GET").uri(uri);
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    let resp = router
        .build_axum_router()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .expect("send");
    let header = |n: &str| {
        resp.headers()
            .get(n)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    };
    (
        resp.status().as_u16(),
        header("traceparent"),
        header("x-trace-id"),
    )
}

const INBOUND: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[tokio::test]
async fn request_span_continues_inbound_trace() {
    let (captured, provider, guard) = otel_capture();
    let (status, tp, xtid) = send(
        build_router(&[("svc/GET/ping.yml", OK)]),
        "/svc/ping",
        Some(INBOUND),
    )
    .await;
    drop(guard);
    provider.force_flush();
    assert_eq!(status, 200);

    let spans = captured.request_spans();
    assert_eq!(spans.len(), 1, "one request span: {spans:?}");
    let span = &spans[0];
    let ctx = &span.span_context;
    assert_eq!(
        ctx.trace_id().to_string(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");
    assert_eq!(span.span_kind, SpanKind::Server);
    assert_eq!(
        attr(span, "http.response.status_code").as_deref(),
        Some("200")
    );
    assert_eq!(attr(span, "http.request.method").as_deref(), Some("GET"));

    // Same trace id in the response; the echoed span id is Ruuter's own
    // span, so downstream callees nest under it.
    assert_eq!(xtid, "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(
        tp,
        format!("00-{}-{}-01", ctx.trace_id(), ctx.span_id()),
        "response traceparent is the request span's context"
    );
}

#[tokio::test]
async fn request_span_without_inbound_traceparent_matches_x_trace_id() {
    let (captured, provider, guard) = otel_capture();
    let (_, tp, xtid) = send(build_router(&[("svc/GET/ping.yml", OK)]), "/svc/ping", None).await;
    drop(guard);
    provider.force_flush();

    let spans = captured.request_spans();
    assert_eq!(spans.len(), 1);
    let ctx = &spans[0].span_context;
    assert_eq!(xtid, ctx.trace_id().to_string());
    assert_eq!(tp, format!("00-{}-{}-01", ctx.trace_id(), ctx.span_id()));
    assert_eq!(
        spans[0].parent_span_id,
        opentelemetry::trace::SpanId::INVALID,
        "a request without traceparent starts a root span"
    );
}

#[tokio::test]
async fn invalid_inbound_traceparent_starts_a_valid_root() {
    let (captured, provider, guard) = otel_capture();
    let (_, tp, xtid) = send(
        build_router(&[("svc/GET/ping.yml", OK)]),
        "/svc/ping",
        Some("garbage"),
    )
    .await;
    drop(guard);
    provider.force_flush();
    let spans = captured.request_spans();
    assert_eq!(spans.len(), 1);
    let ctx = &spans[0].span_context;
    assert!(ctx.is_valid());
    assert_eq!(xtid, ctx.trace_id().to_string());
    assert!(
        tp.starts_with("00-") && tp.len() == 55,
        "valid traceparent: {tp}"
    );
}

#[tokio::test]
async fn server_error_sets_error_status_but_client_error_does_not() {
    let (captured, provider, guard) = otel_capture();
    let router = build_router(&[("svc/GET/fail.yml", FAIL)]);
    let (status, _, _) = send(router, "/svc/fail", Some(INBOUND)).await;
    let (missing, _, _) = send(
        build_router(&[("svc/GET/fail.yml", FAIL)]),
        "/svc/nope",
        None,
    )
    .await;
    drop(guard);
    provider.force_flush();
    assert_eq!(status, 503);
    assert_eq!(missing, 404);

    let spans = captured.request_spans();
    let failed = spans
        .iter()
        .find(|s| attr(s, "http.response.status_code").as_deref() == Some("503"))
        .expect("503 span");
    assert!(
        matches!(failed.status, Status::Error { .. }),
        "{:?}",
        failed.status
    );
    let not_found = spans
        .iter()
        .find(|s| attr(s, "http.response.status_code").as_deref() == Some("404"))
        .expect("404 span");
    assert!(!matches!(not_found.status, Status::Error { .. }));
}

/// Every env-var-driven check lives in this one test: env is process-global
/// and the other tests in this binary run in parallel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exporter_and_resource_follow_otel_env_vars() {
    use observability::{otlp_transport_from_env, OtlpTransport};

    for key in [
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    ] {
        std::env::remove_var(key);
    }
    assert_eq!(otlp_transport_from_env(), OtlpTransport::Grpc);
    std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
    assert_eq!(otlp_transport_from_env(), OtlpTransport::HttpProtobuf);
    std::env::set_var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc");
    assert_eq!(
        otlp_transport_from_env(),
        OtlpTransport::Grpc,
        "per-signal wins"
    );
    std::env::set_var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "http/json");
    assert_eq!(
        otlp_transport_from_env(),
        OtlpTransport::Grpc,
        "unsupported -> grpc"
    );
    std::env::remove_var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL");

    // Resource: OTEL_RESOURCE_ATTRIBUTES applies, service.name from us wins.
    std::env::set_var(
        "OTEL_RESOURCE_ATTRIBUTES",
        "deployment.environment=staging,service.namespace=my-system,service.name=ignored",
    );
    let res = observability::otel_resource("my-gateway");
    let get = |k: &str| {
        res.get(opentelemetry::Key::new(k.to_string()))
            .map(|v| v.to_string())
    };
    assert_eq!(get("service.name").as_deref(), Some("my-gateway"));
    assert_eq!(get("deployment.environment").as_deref(), Some("staging"));
    assert_eq!(get("service.namespace").as_deref(), Some("my-system"));
    assert_eq!(
        get("service.version").as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");

    // gRPC over TLS builds (lazy channel, nothing is dialled here).
    std::env::remove_var("OTEL_EXPORTER_OTLP_PROTOCOL");
    std::env::set_var(
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "https://collector.example:4317",
    );
    observability::build_span_exporter("https://collector.example:4317")
        .expect("https gRPC exporter builds");

    // http/protobuf: POST <endpoint>/v1/traces with OTEL_EXPORTER_OTLP_HEADERS.
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/traces")
        .match_header("authorization", "Bearer s3cret")
        .match_header("content-type", "application/x-protobuf")
        .with_status(200)
        .create_async()
        .await;
    std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", server.url());
    std::env::set_var(
        "OTEL_EXPORTER_OTLP_HEADERS",
        "authorization=Bearer%20s3cret",
    );
    let exporter = observability::build_span_exporter(&server.url()).expect("http exporter");
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();
    {
        use opentelemetry::trace::{Span as _, Tracer as _};
        let mut span = provider.tracer("t").start("probe");
        span.end();
    }
    let flushed = tokio::task::spawn_blocking(move || {
        let r = provider.force_flush();
        let _ = provider.shutdown();
        r
    })
    .await
    .unwrap();
    for r in flushed {
        r.expect("flush");
    }
    for key in [
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_HEADERS",
    ] {
        std::env::remove_var(key);
    }
    mock.assert_async().await;
}

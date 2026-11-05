//! Tracing subscriber + OpenTelemetry setup.
//!
//! Two independent axes:
//!
//! 1. **Log output format** — one of three renderers, driven by
//!    `logging.format` in `AppConfig`; the env var
//!    `RUUTER_LOG_FORMAT=text|json|pretty` overrides so an operator
//!    can flip a running container without editing the config file.
//!
//!    - `text` (default): compact one-line-per-event terminal-first
//!      layout, span noise stripped. See `logging::fmt`.
//!    - `pretty`: `text` + ANSI colours + Unicode markers for
//!      interactive local dev.
//!    - `json`: one OTel-log-compatible JSON object per event —
//!      full field set, recommended for production ingest.
//!
//! 2. **Distributed tracing (OTel spans)** — when
//!    `OTEL_EXPORTER_OTLP_ENDPOINT` is set, the subscriber additionally
//!    exports every `tracing::info_span!` as an OTLP span with the
//!    full field set (span noise trimmed from text rendering does NOT
//!    reach spans — OTLP sees everything). W3C tracecontext
//!    propagation is always installed so downstream calls get the
//!    right `traceparent` regardless of whether an exporter is wired.
//!
//! Reference: `book/src/logging/` (operator-facing chapter) and
//! `book/src/framework/tracing.md`.

use crate::config::{AppConfig, LogFormat};
use crate::logging::fmt::{AllowlistSpanCapture, RuuterPrettyFormat, RuuterTextFormat};
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator, runtime::Tokio, trace::TracerProvider, Resource,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const SERVICE_NAME_DEFAULT: &str = "ruuter-on-rust";

/// Resolve the effective log format: env var wins over config.
fn resolve_format(config_format: LogFormat) -> LogFormat {
    match std::env::var("RUUTER_LOG_FORMAT")
        .ok()
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => LogFormat::Json,
        Some("text") => LogFormat::Text,
        Some("pretty") => LogFormat::Pretty,
        _ => config_format,
    }
}

/// Set once `init` has wired an OTLP exporter. Request handling uses it
/// to decide whether to parent the request span on the W3C trace context
/// and to record OTel-only span fields. When export is off nothing reads
/// or writes those fields, so log output is unchanged.
static OTEL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// `true` when spans are exported over OTLP (see `init`).
pub fn otel_active() -> bool {
    OTEL_ACTIVE.load(Ordering::Relaxed)
}

/// Test hook: integration tests install their own subscriber with an
/// in-memory exporter and flip this on, instead of going through `init`.
#[doc(hidden)]
pub fn set_otel_active_for_tests(active: bool) {
    OTEL_ACTIVE.store(active, Ordering::Relaxed);
}

/// OTLP transport selected by `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` /
/// `OTEL_EXPORTER_OTLP_PROTOCOL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpTransport {
    /// OTLP/gRPC (default, port 4317).
    Grpc,
    /// OTLP/HTTP with protobuf payloads (port 4318, `<endpoint>/v1/traces`).
    HttpProtobuf,
}

/// Resolve the OTLP transport from the standard env vars. The per-signal
/// variable wins. Unknown values fall back to gRPC with a warning on stderr
/// (the subscriber is not installed yet when this runs).
pub fn otlp_transport_from_env() -> OtlpTransport {
    let raw = std::env::var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL"))
        .unwrap_or_default();
    match raw.trim() {
        "" | "grpc" => OtlpTransport::Grpc,
        "http/protobuf" => OtlpTransport::HttpProtobuf,
        other => {
            eprintln!(
                "OTEL_EXPORTER_OTLP_PROTOCOL={other:?} is not supported (grpc, http/protobuf); using grpc"
            );
            OtlpTransport::Grpc
        }
    }
}

/// Resource for exported spans: the SDK defaults (which include
/// `OTEL_RESOURCE_ATTRIBUTES` via the env detector) merged with
/// `service.name` / `service.version` from this process, which win.
pub fn otel_resource(service_name: &str) -> Resource {
    Resource::default().merge(&Resource::new(vec![
        KeyValue::new("service.name", service_name.to_string()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ]))
}

/// Build the OTLP span exporter for `endpoint` with the transport from
/// [`otlp_transport_from_env`].
///
/// TLS is provided by OTLP/HTTP, which reuses the `reqwest` client and its
/// TLS stack; the gRPC exporter is plaintext (tonic is built without TLS
/// to keep `rustls-pemfile` out of the tree). An `https://` endpoint with
/// gRPC therefore warns once and is expected to fail on export.
pub fn build_span_exporter(endpoint: &str) -> Result<opentelemetry_otlp::SpanExporter, String> {
    // Endpoint, OTEL_EXPORTER_OTLP_HEADERS and OTEL_EXPORTER_OTLP_TIMEOUT are
    // read by opentelemetry-otlp itself; header values are never logged here.
    match otlp_transport_from_env() {
        OtlpTransport::Grpc => {
            if endpoint.starts_with("https://") {
                eprintln!(
                    "OTEL_EXPORTER_OTLP_ENDPOINT is https:// but the gRPC exporter has no TLS; \
                     set OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf for a TLS endpoint"
                );
            }
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .map_err(|e| e.to_string())
        }
        OtlpTransport::HttpProtobuf => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .with_endpoint(endpoint)
            .with_http_client(reqwest::Client::new())
            .build()
            .map_err(|e| e.to_string()),
    }
}

/// Try to build the OTLP exporter and pipeline. Returns `None` on
/// error (falls back to local logging). Kept in its own fn so the
/// two format branches can share it.
fn try_build_otel(endpoint: &str) -> Option<TracerProvider> {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());
    let exporter = build_span_exporter(endpoint)
        .map_err(|err| {
            eprintln!(
                "OTel exporter init failed ({}); falling back to local logging only",
                err
            );
        })
        .ok()?;
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, Tokio)
        .with_resource(otel_resource(&service_name))
        .build();
    OTEL_ACTIVE.store(true, Ordering::Relaxed);
    Some(provider)
}

/// Make `span` a child of the remote context carried by a W3C
/// `traceparent` value. Invalid values leave the span as a new root.
pub fn set_parent_from_traceparent(span: &tracing::Span, traceparent: &str) {
    let mut carrier = HashMap::new();
    carrier.insert("traceparent".to_string(), traceparent.to_string());
    let cx = TraceContextPropagator::new().extract(&carrier);
    if cx.span().span_context().is_valid() {
        span.set_parent(cx);
    }
}

/// The `traceparent` of `span` itself (`00-<trace>-<span>-<flags>`), or
/// `None` when no OTel layer is installed for it.
pub fn traceparent_of(span: &tracing::Span) -> Option<String> {
    let cx = span.context();
    let sc = cx.span().span_context().clone();
    if !sc.is_valid() {
        return None;
    }
    Some(format!(
        "00-{}-{}-{:02x}",
        sc.trace_id(),
        sc.span_id(),
        sc.trace_flags().to_u8()
    ))
}

/// Build the fmt layer for the compact terminal formatter (text or
/// pretty variant). Both need the same span-field capture layer
/// installed alongside so span fields survive into event rendering.
fn compact_fmt_layer<S>(
    ansi: bool,
) -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::DefaultFields,
    RuuterTextFormat,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().event_format(RuuterTextFormat { ansi })
}

fn pretty_fmt_layer<S>() -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::DefaultFields,
    RuuterPrettyFormat,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().event_format(RuuterPrettyFormat::new())
}

/// Initialize tracing. Called from main.rs AFTER config load so the
/// format toggle is honoured. Returns `Some(TracerProvider)` if OTLP
/// export was wired (caller keeps it alive and shuts it down on exit);
/// `None` if only the local subscriber was installed.
pub fn init(config: &AppConfig) -> Option<TracerProvider> {
    let format = resolve_format(config.logging.format);
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty());

    // Concrete type per branch avoids `Box<dyn Layer>` which needs a
    // trait bound the current tracing-subscriber release doesn't
    // provide. Small code duplication is cheaper than a downstream
    // helper trait.
    let provider: Option<TracerProvider> = match (&format, endpoint.as_deref()) {
        (LogFormat::Text, Some(url)) => {
            let provider = try_build_otel(url);
            if let Some(ref p) = provider {
                let service = std::env::var("OTEL_SERVICE_NAME")
                    .unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());
                let tracer = p.tracer(service);
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(AllowlistSpanCapture)
                    .with(compact_fmt_layer(false))
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(AllowlistSpanCapture)
                    .with(compact_fmt_layer(false))
                    .init();
            }
            provider
        }
        (LogFormat::Text, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(AllowlistSpanCapture)
                .with(compact_fmt_layer(false))
                .init();
            None
        }
        (LogFormat::Pretty, Some(url)) => {
            let provider = try_build_otel(url);
            if let Some(ref p) = provider {
                let service = std::env::var("OTEL_SERVICE_NAME")
                    .unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());
                let tracer = p.tracer(service);
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(AllowlistSpanCapture)
                    .with(pretty_fmt_layer())
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(AllowlistSpanCapture)
                    .with(pretty_fmt_layer())
                    .init();
            }
            provider
        }
        (LogFormat::Pretty, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(AllowlistSpanCapture)
                .with(pretty_fmt_layer())
                .init();
            None
        }
        (LogFormat::Json, Some(url)) => {
            let provider = try_build_otel(url);
            if let Some(ref p) = provider {
                let service = std::env::var("OTEL_SERVICE_NAME")
                    .unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());
                let tracer = p.tracer(service);
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_current_span(true)
                            .with_span_list(false),
                    )
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(
                        tracing_subscriber::fmt::layer()
                            .json()
                            .with_current_span(true)
                            .with_span_list(false),
                    )
                    .init();
            }
            provider
        }
        (LogFormat::Json, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_current_span(true)
                        .with_span_list(false),
                )
                .init();
            None
        }
    };

    provider
}

/// Best-effort tracer shutdown. Flush pending spans before exit.
pub fn shutdown(provider: Option<TracerProvider>) {
    if let Some(p) = provider {
        let _ = p.shutdown();
    }
}

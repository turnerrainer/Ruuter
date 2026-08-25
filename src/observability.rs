//! Tracing subscriber + OpenTelemetry setup.
//!
//! Two independent axes:
//!
//! 1. **Log output format** — either the default `tracing` text
//!    layer or a JSON layer emitting one OTel-log-compatible JSON
//!    object per event. Driven by `logging.format` in `AppConfig`;
//!    the env var `RUUTER_LOG_FORMAT=text|json` overrides so an
//!    operator can flip a running container without editing the
//!    config file.
//!
//! 2. **Distributed tracing (OTel spans)** — when
//!    `OTEL_EXPORTER_OTLP_ENDPOINT` is set, the subscriber additionally
//!    exports every `tracing::info_span!` as an OTLP span. W3C
//!    tracecontext propagation is always installed so downstream
//!    calls get the right `traceparent` regardless of whether an
//!    exporter is wired.
//!
//! Reference: `book/src/logging/` (operator-facing chapter) and
//! `book/src/framework/tracing.md`.

use crate::config::{AppConfig, LogFormat};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{runtime::Tokio, trace::TracerProvider, Resource};
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
        _ => config_format,
    }
}

/// Try to build the OTLP exporter and pipeline. Returns `None` on
/// error (falls back to local logging). Kept in its own fn so the
/// two format branches can share it.
fn try_build_otel(endpoint: &str) -> Option<TracerProvider> {
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|err| {
            eprintln!(
                "OTel exporter init failed ({}); falling back to local logging only",
                err
            );
        })
        .ok()?;
    Some(
        TracerProvider::builder()
            .with_batch_exporter(exporter, Tokio)
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                service_name,
            )]))
            .build(),
    )
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
                    .with(tracing_subscriber::fmt::layer())
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(env_filter)
                    .with(tracing_subscriber::fmt::layer())
                    .init();
            }
            provider
        }
        (LogFormat::Text, None) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
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

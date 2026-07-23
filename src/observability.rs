//! OpenTelemetry tracing setup.
//!
//! Behavior:
//! - If `OTEL_EXPORTER_OTLP_ENDPOINT` is set, build an OTLP tracer
//!   pipeline and add a `tracing_opentelemetry` layer to the
//!   subscriber. The service name is taken from `OTEL_SERVICE_NAME`
//!   (default `ruuter-on-rust`).
//! - If the env var is unset, no exporter is built — local dev works
//!   without any OTel infrastructure.
//!
//! Trace propagation is W3C TraceContext (`traceparent`/`tracestate`)
//! by default — matching what every other Buerostack component speaks.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{runtime::Tokio, trace::TracerProvider, Resource};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const SERVICE_NAME_DEFAULT: &str = "ruuter-on-rust";

/// Initialize tracing. Returns `Some(TracerProvider)` if OTel was
/// wired (caller should keep it alive for the lifetime of the
/// process and shut it down on exit); `None` if only the fmt layer
/// was installed.
pub fn init() -> Option<TracerProvider> {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let fmt_layer = tracing_subscriber::fmt::layer();

    match endpoint {
        Some(url) if !url.is_empty() => {
            // W3C TraceContext propagation (interop with the other Buerostack components).
            opentelemetry::global::set_text_map_propagator(
                opentelemetry_sdk::propagation::TraceContextPropagator::new(),
            );

            let service_name = std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| SERVICE_NAME_DEFAULT.to_string());

            let exporter = match opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&url)
                .build()
            {
                Ok(e) => e,
                Err(err) => {
                    eprintln!(
                        "OTel exporter init failed ({}); falling back to fmt-only logging",
                        err
                    );
                    tracing_subscriber::registry()
                        .with(env_filter)
                        .with(fmt_layer)
                        .init();
                    return None;
                }
            };

            let provider = TracerProvider::builder()
                .with_batch_exporter(exporter, Tokio)
                .with_resource(Resource::new(vec![KeyValue::new(
                    "service.name",
                    service_name.clone(),
                )]))
                .build();
            let tracer = provider.tracer(service_name);
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .with(otel_layer)
                .init();

            Some(provider)
        }
        _ => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .init();
            None
        }
    }
}

/// Best-effort tracer shutdown. Flush pending spans before exit.
pub fn shutdown(provider: Option<TracerProvider>) {
    if let Some(p) = provider {
        let _ = p.shutdown();
    }
}

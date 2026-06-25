# 014 — Wire OpenTelemetry exporter

**Status**: BACKLOG.
**Severity**: MEDIUM (architectural alignment — Buerostack expects
distributed tracing across all components per ARCHITECTURE.md §8).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.

## What's wrong

`Cargo.toml` already declares:
```toml
opentelemetry = "0.27"
opentelemetry-otlp = "0.27"
tracing-opentelemetry = "0.28"
```

…but `src/main.rs` only initializes `tracing-subscriber` with the
plain `fmt` layer. There's no OTLP exporter, no propagation, no
spans tied to a tracer. So traces stop at Ruuter's boundary — a
request that hits Ruuter → Resql → DB doesn't produce a single
correlated trace.

The Buerostack architecture's "observable security" / defense-in-
depth posture assumes complete distributed tracing.

## Fix

1. Construct an `opentelemetry_otlp::new_pipeline().tracing()` with
   sensible defaults read from env vars
   (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`).
2. Add a `tracing_opentelemetry::layer().with_tracer(...)` to the
   subscriber registry alongside the existing `fmt::layer()`.
3. Propagate inbound `traceparent`/`tracestate` headers (axum middleware)
   and inject them into outbound `reqwest` calls in `HttpClient`.
4. Make exporter setup conditional on an env var being set — local
   dev shouldn't require an OTLP endpoint.

## Verification

- `OTEL_EXPORTER_OTLP_ENDPOINT=http://jaeger:4317 docker-compose up`
  with a Jaeger UI; hit `/samples/javascript/array-operations` and
  observe a trace with the inbound request span.
- Verify outbound `http` steps propagate `traceparent`.
- Verify local dev (no env var) still works without an exporter.

## Why this is generic

OTel is a standard concern across every Buerostack component. No
service-specific code.

# Traceparent & OpenTelemetry

W3C tracecontext propagation is on by default. OTLP span export is opt-in.

## What ships automatically

- **On every response**: `traceparent` header (either echoed from the request or freshly generated) plus `X-Trace-Id` extracted from the trace id.
- **On every outbound `http` step call**: the DSL's `traceparent` is forwarded upstream unless the DSL sets one explicitly in `headers:`.

No configuration required for either.

## Verification

```
$ curl -sSD - -H 'Traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' \
    http://localhost:8080/samples/basic/hello -o /dev/null | grep -iE 'trace'
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
x-trace-id: 4bf92f3577b34da6a3ce929d0e0e4736
```

Request without a `traceparent` → framework generates one; both response headers are populated with the fresh id.

## OTLP export

Enable by setting environment variables:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
OTEL_SERVICE_NAME=ruuter-on-rust                # default; override if you run multiple instances
```

Without `OTEL_EXPORTER_OTLP_ENDPOINT`, no exporter is built — Ruuter still generates traceparent locally, just doesn't ship spans anywhere.

## Traceparent format

W3C standard: `<version>-<trace_id>-<span_id>-<flags>`. Ruuter generates `01` (sampled) for the flags on new traces.

```
00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
   └── 32-hex trace id ───────────┘ └─ 16-hex span ─┘ └─ flags
```

The 32-hex trace id is what shows up as `X-Trace-Id`.

## Replay and trace correlation

Framework-level `Idempotency-Key` handling was removed in v0.7.0.
When a DSL implements the [DSL idempotency pattern](../dsl/idempotency-pattern.md)
and short-circuits to a cached response, a fresh traceparent still
fires — correlate the original and replay via whichever dedup key
the DSL wrote into state, not the trace id.

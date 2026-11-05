# Traceparent & OpenTelemetry

W3C tracecontext propagation is on by default. OTLP span export is opt-in.

## What ships automatically

- **On every response**: `traceparent` header (either echoed from the request or freshly generated) plus `X-Trace-Id` extracted from the trace id.
- **On every outbound `http` step call**: the DSL's `traceparent` is forwarded upstream unless the DSL sets one explicitly in `headers:`.

No configuration required for either.

## Verification

Request:

```bash
curl -sSD - -H 'Traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' \
     http://localhost:8080/samples/basic/hello -o /dev/null | grep -iE 'trace'
```

Response header lines:

```http
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

Every variable below is optional and only read when `OTEL_EXPORTER_OTLP_ENDPOINT` is set:

| Variable | Effect |
|---|---|
| `OTEL_SERVICE_NAME` | `service.name` of the exported spans (default `ruuter-on-rust`) |
| `OTEL_RESOURCE_ATTRIBUTES` | Extra resource attributes, e.g. `deployment.environment=staging,service.namespace=my-system`; `service.name` and `service.version` from Ruuter win |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` (default, port 4317) or `http/protobuf` (port 4318, POST `<endpoint>/v1/traces`); `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` wins |
| `OTEL_EXPORTER_OTLP_HEADERS` | `key=value,key2=value2` (values URL-encoded) sent on every export, e.g. `authorization=Bearer%20<token>`; never logged |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Export timeout in seconds |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | Standard sampler selection (default `parentbased_always_on`) |

An `https://` endpoint uses TLS with the bundled webpki root certificates.

### What gets exported

One SERVER span per request, named `HTTP <METHOD> /<path>`:

- parent: the inbound `traceparent`, or none (new root);
- attributes: `http.request.method`, `http.route`, `http.response.status_code`, `dsl.project`, `client.address`, `trace_id`;
- status: Error for 5xx responses; 4xx is not an error;
- log events emitted during the request become span events with the same fields as the log line.

With export on, the `traceparent` Ruuter propagates (DSL context, `http` steps, response header) is the request span's own context: the trace id is the same as the inbound one, the span id is Ruuter's. Downstream services therefore nest under Ruuter's span, and `X-Trace-Id`, the log `trace_id` and the exported trace id are equal. Without export, the inbound `traceparent` is propagated and echoed verbatim, as before.

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

# Errors & trace correlation

Three related mechanisms: what a failure looks like in the
**response body** the caller sees, what it looks like in the
**log stream**, and how one `trace_id` links every event of
one request into a single browsable timeline.

## Response body

When a request fails, the JSON error body always carries:

1. **Which step** failed (name + type).
2. **In which project.**
3. **The full source-chain of causes** — top-level error, plus
   `-> caused by: …` for every hop of `std::error::Error::source()`.

Example — a DSL step named `call_upstream` that runs an
`http.get` to an unresolvable hostname:

```json
{
  "error": "step 'call_upstream' (http) in project 'consignment' failed -> caused by: HTTP error: error sending request for url (http://multiplexer:8083/...) -> caused by: error sending request for url (...) -> caused by: client error (Connect) -> caused by: dns error -> caused by: failed to lookup address information: Temporary failure in name resolution"
}
```

Before this behaviour landed (issues #28 + #29), the same failure
returned only:

```json
{"error":"HTTP error: error sending request for url (...)"}
```

— no step name, no DSL context, no underlying DNS/TCP/TLS
diagnostic. Debugging required scanning server logs and hoping
`meaningful_errors` was enabled.

The chain is bounded to 5 hops so a runaway wrapper chain can't
fill the response line.

### Sensitive-info consideration

The enriched shape can leak infrastructure names into the API
surface (upstream hostnames, internal step names, DNS diagnostics).
If your Ruuter is public-facing and you need to keep those
private, wrap the endpoint in a DSL guard that rewrites the
response body — the framework's default is "helpful over
opaque" because most failures need this detail for debugging.

## Error rendering

Ruuter emits two error-related events on step failure:

**Primary ERROR line** (always emitted):

```json
{
  "level": "ERROR",
  "target": "ruuter_on_rust::steps::engine",
  "fields": {
    "message": "step failed",
    "dsl.step": "fetch_user",
    "dsl.step.type": "http",
    "duration_ms": 42.31,
    "error": "upstream status 500 not in http_codes_allow_list",
    "cause_chain": ""
  },
  "span": { ... request span attributes ... }
}
```

**Second WARN line** (only when `logging.meaningful_errors: true`):

```json
{
  "level": "WARN",
  "target": "ruuter_on_rust::steps::engine",
  "fields": {
    "message": "step failed (underlying cause)",
    "dsl.step": "fetch_user",
    "cause": "connection refused"
  },
  "span": { ... request span attributes ... }
}
```

## The two error knobs

### `meaningful_errors`

- **Off** (default): one primary ERROR line, `error` field
  carries the top-level `Display`.
- **On**: primary ERROR line plus a second WARN line with just
  the underlying `source().to_string()`. Useful when the top-
  level error wraps ("upstream status 500") but the underlying
  cause ("connection refused") is what the operator actually
  needs to see.

Java Ruuter had the same flag; Ruuter-on-Rust preserves the
name and semantics.

### `print_stack_trace`

- **Off** (default): `cause_chain` field is empty.
- **On**: `cause_chain` field on the primary ERROR line renders
  the full `source()` chain, bounded to 5 hops, formatted as:

```
-> caused by: connection refused -> caused by: timeout after 5s -> caused by: no route to host
```

Bounded on purpose. Runaway chains do exist (framework-internal
wrapping in async runtimes), and unbounded chains would fill a
log line without adding signal.

## What Ruuter does NOT emit

- **Rust `Backtrace`**. Modern observability uses OTLP span
  exports for the "which code path fired" question; per-line
  backtraces have poor cost/signal ratio in production logs.
- **Panic messages**. Panics are process-level failures — the
  container runtime catches them via stderr, the log-aggregation
  layer picks them up as normal stderr lines, no framework
  handling needed.
- **`?`-unwind step names**. Rust's `?` operator doesn't
  produce a step-stack analogue to a Python traceback; the
  `dsl.step` + `cause_chain` fields are the equivalent.

## Trace correlation lifecycle

The framework computes exactly one `trace_id` per request. It
appears on every request-scoped event and on the response
headers.

### Lifecycle

1. **Request enters `handle_request`.** The framework looks for
   an inbound `traceparent` header.
2. **If present**, the header value is adopted verbatim. The
   32-hex trace id is extracted for log fields.
3. **If absent**, a fresh W3C-compliant `traceparent` is
   generated (`00-<32-hex trace_id>-<16-hex span_id>-01`).
   The generated traceparent is INJECTED into the request
   headers so every downstream reader (guards, steps, DSL
   context) sees the same value.
4. **Request span** is opened with `trace_id` as a field.
   Every log line emitted inside the request inherits it via
   `tracing`'s span-context propagation.
5. **DSL execution context** carries the traceparent; the HTTP
   step forwards it as `traceparent` on every outbound call.
6. **Response** carries `traceparent: <same>` and
   `X-Trace-Id: <same-32-hex>` headers.

The same 32-hex `trace_id` therefore appears:

- In the response headers the caller receives.
- On every log line emitted inside the request.
- On every OTLP span exported by the OTel exporter (when wired
  via `OTEL_EXPORTER_OTLP_ENDPOINT`).
- On every outbound `http.<verb>` request's `traceparent`
  header, so downstream services log the same id.

### Verification

```bash
curl -sSD - -H 'traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' \
     http://localhost:8080/samples/basic/hello -o /dev/null | grep -iE 'trace'
```

Response headers:

```
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
x-trace-id: 4bf92f3577b34da6a3ce929d0e0e4736
```

Server logs (JSON):

```bash
$ docker compose logs ruuter | jq -c '. | select(.fields.trace_id == "4bf92f3577b34da6a3ce929d0e0e4736")'
```

returns every event fired inside that request: access log,
per-step DEBUG lines (if `step_timing`), DSL `log:` events, any
errors.

### Correlating client-side and server-side

Any tracing-aware client can generate a `traceparent` and pass
it to Ruuter. Ruuter adopts it, decorates all its log lines,
and forwards to downstream services. The result: one shared
trace id across the whole request chain.

For clients that don't do W3C tracecontext natively, the
returned `X-Trace-Id` header lets the client log the id it got
back and later search server logs by that value.

## OTel span export vs log lines

They complement each other:

- **Logs answer "what happened, in what order?"** — timestamped,
  human-readable, greppable.
- **Spans answer "how long, in what shape?"** — a request tree
  with per-span duration, easy to visualise in Jaeger / Tempo /
  Datadog / Grafana Traces.

Both share the `trace_id`, so an operator jumping from a slow
span to the log lines behind it uses the same id string in the
UI's filter box.

To enable span export:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
OTEL_SERVICE_NAME=ruuter-on-rust
```

See [Traceparent & OpenTelemetry](../framework/tracing.md) for
the full trace-propagation story.

## Failure modes that DON'T produce ERROR lines

Not every non-2xx response is a framework error. Deliberate:

- **404 (route not found)**. Just an INFO access-log line with
  status 404. Not an error — the DSL tree simply doesn't
  contain that route.
- **405 (method not allowed)**. Ditto.
- **412 (precondition required)** from the optimistic-concurrency
  gate. Ditto.
- **DSL returning a 4xx or 5xx via `return: { status: 500, ... }`**.
  Ditto — the DSL decided the response; the framework's role
  was to deliver it.

Framework ERRORs are reserved for "the framework couldn't do
what the DSL asked" — HTTP step exhausted its allow-list, JS
expression failed to compile, config-declared exception DSL
itself errored, etc.

## Cross-links

- [Configuration reference](./configuration.md#meaningful_errors)
  and [`print_stack_trace`](./configuration.md#print_stack_trace).
- [Field vocabulary — error fields](./fields.md#error-fields).
- [Traceparent & OpenTelemetry](../framework/tracing.md).
- [Recipes / investigating a request](./recipes.md#investigating-a-specific-request).

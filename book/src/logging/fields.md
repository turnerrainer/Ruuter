# Field vocabulary

Ruuter uses OpenTelemetry HTTP Semantic Conventions where they
exist. Rationale: dashboards written for one OTel-shaped service
should just work here.

## Semantic-convention fields (OTel)

These field names are stable across the OTel ecosystem. Dashboards
keying on them are portable across services and language runtimes.

| Ruuter field | OTel convention | Emitted by | Notes |
|---|---|---|---|
| `http.request.method` | ✅ | request span, access log, outbound HTTP DEBUG | e.g. `GET`, `POST` |
| `http.route` | ✅ | request span, access log | Path as received; NOT the DSL file id |
| `http.response.status_code` | ✅ | access log, outbound HTTP DEBUG | Integer |
| `url.full` | ✅ | outbound HTTP DEBUG | Redacted / capped for cardinality safety |
| `client.address` | ✅ | request span, access log | IP; requires proxy trust config for XFF promotion |
| `duration_ms` | (custom, but widely used) | access log, step DEBUG | `f64` milliseconds |

## Trace-correlation fields

Every request-scoped event carries these:

| Field | Format | Source |
|---|---|---|
| `trace_id` | 32-hex lowercase | Extracted from adopted `traceparent`, or generated at request entry |
| `otel.name` | `HTTP <METHOD> <path>` | Attached to the request span for OTLP exporters (Jaeger / Tempo / Datadog show this as the span name) |

The `trace_id` field appears both directly on the log line's
`fields` (so text-format grep works) AND on the enclosing span
(so JSON-format `span` object carries it too). Redundant on
purpose — either shape is greppable.

## Ruuter DSL fields

These describe DSL-level state and are prefixed with `dsl.` so
dashboards can group / filter by prefix.

| Field | Format | Source |
|---|---|---|
| `dsl.project` | string | First URL path segment; cardinality-safe |
| `dsl.step` | string | Step name from the YAML (e.g. `log_start`, `fetch_user`) |
| `dsl.step.type` | string | Step type — one of `assign`, `http`, `return`, `log`, `switch`, `template`, `state`, `iterate`, `ws_send`, `single_flight`, `http_mock`, `declaration`; `skip` when `skip: true` fired |
| `dsl.next.step` | string | Next step target (`-` when the engine falls through to source-order next) |
| `dsl.total_steps` | integer | On `DSL run started` bracket line |
| `dsl.first_step` | string | On `DSL run started` bracket line |
| `dsl.steps_ran` | integer | On `DSL run completed` bracket line |
| `terminated_by` | string | On `DSL run completed`: one of `return`, `end_of_steps`, `iteration_cap`, `error` |
| `terminating_step` | string | On `DSL run completed`, `terminated_by=return` |
| `failed_step` | string | On `DSL run completed`, `terminated_by=error` |
| `dsl.log` | string | Interpolated message body from the `log:` DSL step |
| `attrs` | rendered `k=v` pairs | Per-step-type context on `Executed` INFO lines (issue #37). See per-type field list in [Configuration](./configuration.md#log_step_executions). |

## Outbound HTTP fields

Emitted only when the corresponding config flag is on. All are
redacted / capped before emission — see [Redaction](./redaction.md).

| Field | Config gate |
|---|---|
| `http.request.body` | `logging.display_request_content` |
| `http.request.headers` | `logging.display_request_content` |
| `http.response.body` | `logging.display_response_content` |
| `http.response.headers` | `logging.display_response_content` |

## Error fields

| Field | Level | Source |
|---|---|---|
| `error` | ERROR (step failure) | The top-level error's `Display` |
| `cause_chain` | ERROR (when `print_stack_trace` on) | ` -> caused by: X -> caused by: Y`, bounded to 5 hops |
| `cause` | WARN (when `meaningful_errors` on) | Underlying `source().to_string()` on a second WARN line |
| `skipped` | DEBUG (with `step_timing`) | `true` if the step's `skip: true` fired |

## The request span

Every HTTP request opens a `tracing::info_span!("http_request", …)`.
Every child event inherits these fields. When the JSON formatter
is active, the span's fields appear in the `span` object of the
JSON output; the OTLP exporter surfaces them as span attributes.

| Span field | Value | Purpose |
|---|---|---|
| `name` | `http_request` | Fixed identifier; use in Loki/Grafana as `span.name="http_request"` |
| `otel.name` | `HTTP <METHOD> <path>` | Human-readable span name in Jaeger/Tempo/Datadog |
| `http.request.method` | e.g. `GET` | OTel Semantic Convention |
| `http.route` | URI path | OTel Semantic Convention |
| `dsl.project` | first path segment | Project namespace |
| `client.address` | resolved caller IP | See [Reverse-proxy trust](../config/proxy-trust.md) |
| `trace_id` | 32-hex trace id | For grep-based correlation |

## Field-naming principles (design notes)

- **Dots, not underscores.** OTel Semantic Conventions use dots
  (`http.request.method`). Rust's `tracing` accepts either; we
  use dots for the whole vocabulary so grep patterns are uniform.
- **Prefix by concept.** `http.`, `url.`, `client.` follow OTel.
  `dsl.` is Ruuter-specific. Never mix — a `dsl.http.method`
  would be un-greppable across the two prefixes.
- **Same field, same name.** `http.request.method` on the access
  log and on the outbound HTTP DEBUG line are the SAME field. A
  Loki `sum by (http.request.method)` sees both.

## Cross-links

- [Output formats](./formats.md) — where fields appear (JSON
  `fields`/`span` vs text `k=v`).
- [Configuration reference](./configuration.md) — which flags
  gate which fields.
- [Redaction](./redaction.md) — how header/body fields are
  sanitised before emission.
- [Errors & trace correlation](./errors.md) — trace_id lifecycle.

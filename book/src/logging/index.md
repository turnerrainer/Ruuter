# Overview

Ruuter's log surface is designed for production observability first.
Every event is a `tracing` structured record; every request-scoped
event carries the trace id, HTTP semantic-convention fields, and
DSL-level context (`dsl.project`, `dsl.step`); secrets are redacted
before they enter a log line; and the whole thing serialises to JSON
when you flip one switch.

This section is the operator's contract with the log stream. If a
field is documented here, dashboards and alerts can rely on it. If
a knob is documented here, flipping it does what the row says — the
four Java-parity `logging.*` flags have been wired end-to-end (they
were previously accepted-but-inert; that regression closed with
this section), and the per-step INFO execution trail matches Java
Ruuter's `LoggingUtils.logStep()` behaviour by default (issue #37 —
see [Java Ruuter parity](./java-parity.md#per-step-execution-trail-issue-37)).

## What's in this section

- **[Output formats](./formats.md)** — text vs JSON, when to pick
  which, worked examples of both.
- **[Field vocabulary](./fields.md)** — every semantic-convention
  field Ruuter emits, plus DSL-specific extensions.
- **[Configuration reference](./configuration.md)** — every knob
  under `logging:` with defaults and effects.
- **[Redaction & log-injection defence](./redaction.md)** — how
  the framework strips secrets and hostile newlines before any
  value hits a log line.
- **[Errors & trace correlation](./errors.md)** — error rendering
  knobs and how one `trace_id` links the whole request path.
- **[Recipes](./recipes.md)** — production, dev, and hardening
  playbooks.
- **[Java Ruuter parity](./java-parity.md)** — MDC field-by-field
  translation for teams porting Java dashboards.

## Design goals

- **OTel-native.** Field names track the OpenTelemetry HTTP
  Semantic Conventions where they exist (`http.request.method`,
  `http.route`, `http.response.status_code`, `client.address`) so
  dashboards written for one OTel-shaped service work here without
  per-service field mapping.
- **Correlatable.** Every request opens an `info_span!` that
  decorates every child log line with `trace_id`, `http.route`,
  `dsl.project`, and `client.address`. One request → one trace id
  in the response header, the log lines, and the exported OTLP
  span.
- **Redact by default.** A short list of common secret-bearing
  header names (`authorization`, `cookie`, …) and body-field
  names (`password`, `token`, …) is replaced with `"[REDACTED]"`
  before any log emission. Operators extend the lists, never
  shorten them by accident.
- **Cardinality-safe.** Access-log fields are the URL path and the
  DSL project name, not the concrete request path with dynamic
  segments interpolated. Java Ruuter used the wildcard path,
  which was already the safe choice; Ruuter-on-Rust matches.
- **Legible on the happy path.** Default configuration emits one
  INFO access-log line per request and one `Executed` INFO line
  per DSL step (with step-type-specific `attrs`) — the Java-
  parity execution trail (issue #37). The request span already
  frames each run via `trace_id`, so explicit `DSL run started` /
  `DSL run completed` bracket lines are opt-in via `log_dsl_runs`.
  Every additional verbosity axis (per-step DEBUG, outbound
  request/response bodies, error cause chains) is opt-in via a
  single config field. High-QPS DSLs can turn off
  `log_step_executions` to drop the per-step trail entirely; the
  access log and OTel spans remain independent.

## The six log event families

Every log line Ruuter emits falls into one of these buckets:

| Family | Level | Emitted when | Structured fields |
|---|---|---|---|
| **Boot / lifecycle** | INFO / ERROR | Startup, DSL load, config resolution, listener bind, shutdown | free-form message (no request context) |
| **Access log** | INFO | Once per completed HTTP request | `http.request.method`, `http.route`, `http.response.status_code`, `duration_ms`, `dsl.project`, `client.address`, `trace_id` |
| **DSL run bracket** | INFO | Start and end of every DSL invocation (gated by `logging.log_dsl_runs`, opt-in) | `dsl.project`, `dsl.total_steps`, `dsl.first_step` (start); `dsl.steps_ran`, `duration_ms`, `terminated_by` (`return` / `end_of_steps` / `iteration_cap` / `error`), plus `terminating_step` + `http.response.status_code` on `return`, `failed_step` on `error` (end) |
| **DSL step** | INFO `Executed` (gated by `logging.log_step_executions`, on by default) / DEBUG (`step_timing`) | Every completed step for the INFO trail | `dsl.step`, `dsl.step.type`, `duration_ms`, `dsl.next.step`, plus a rendered `attrs` field with step-type-specific context (see [Configuration reference](./configuration.md#log_step_executions)); `log:` step payload appears as `attrs.msg` on this same line |
| **Outbound HTTP** | DEBUG | Only when `display_request_content` / `display_response_content` is on | `http.request.method`, `url.full`, `http.request.body`, `http.request.headers`, `http.response.status_code`, `http.response.body`, `http.response.headers` (redacted, capped) |
| **Error** | ERROR / WARN | Step or DSL fails | `dsl.step`, `dsl.step.type`, `duration_ms`, `error`, `cause_chain` (with `print_stack_trace`), second WARN `cause` line (with `meaningful_errors`) |

All six families are wrapped in the request span when they fire
inside a request, so any of them can be filtered by `trace_id`.

## Where to look next

- New to Ruuter observability? Read [Output formats](./formats.md)
  and [Recipes](./recipes.md) in that order.
- Porting from Java Ruuter? Skip to [Java Ruuter parity](./java-parity.md).
- Building a dashboard? Read [Field vocabulary](./fields.md).
- Hardening for prod? Read [Redaction](./redaction.md) and the
  hardening recipe in [Recipes](./recipes.md#hardening--extend-redaction).

## Cross-links outside this section

- [Traceparent & OpenTelemetry](../framework/tracing.md) — how
  trace ids propagate to downstream services.
- [Environment variables](../ops/env.md) — `RUUTER_LOG_FORMAT`, `RUST_LOG`.
- [Reverse-proxy trust](../config/proxy-trust.md) — why
  `client.address` may be the socket peer instead of an XFF value.
- [Inert fields](../config/inert-fields.md) — the four `logging.*`
  fields listed there are now wired, not inert.

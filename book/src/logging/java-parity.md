# Java Ruuter parity

For teams porting Java Ruuter dashboards, alerts, or operational
runbooks to Ruuter-on-Rust. This page maps every logging-related
Java concept to its Rust equivalent.

## MDC field translation

Java Ruuter carried per-request context via SLF4J's MDC (see
`src/main/java/ee/buerokratt/ruuter/util/LoggingUtils.java`
upstream). Ruuter-on-Rust surfaces the same data via
`tracing`'s span-context propagation.

| Java MDC field | Ruuter-on-Rust equivalent | Notes |
|---|---|---|
| `traceId` | `trace_id` | 32-hex lowercase, extracted from adopted or generated W3C `traceparent`. Appears on the request span AND on every access-log line. |
| `spanId` | (in OTel span exports only) | Ruuter's per-request span carries a span id via `tracing-opentelemetry`; not surfaced in the text/JSON log line (redundant with `trace_id` for correlation purposes). |
| `requestAuthorIp` | `client.address` | Resolved caller IP. Direct socket peer unless `proxy.trusted` promotes X-Forwarded-For. |
| `stepType` | `dsl.step.type` | Step type — one of `assign`, `http`, `return`, `log`, `switch`, `template`, `state`, `iterate`, `ws_send`, `single_flight`, `http_mock`, `declaration`. |
| `requestTo` | `url.full` | Outbound HTTP DEBUG line only (needs `display_request_content`). |
| `requestContent` | `http.request.body` | Outbound HTTP DEBUG line only (needs `display_request_content`). Redacted per `redact_body_fields`, capped at `max_body_bytes`. |
| `responseContent` | `http.response.body` | Outbound HTTP DEBUG line only (needs `display_response_content`). Same redaction / cap. |
| `responseCode` | `http.response.status_code` | On access log AND outbound HTTP DEBUG. |
| `responseInMs` | `duration_ms` | On access log (whole request) or step DEBUG (per step). |

## Config-flag translation

Java Ruuter's `logging:` block had four flags. All four are
preserved by name and semantics in Ruuter-on-Rust:

| Java flag | Rust flag | Wired end-to-end? | Notes |
|---|---|---|---|
| `logging.displayRequestContent` | `logging.display_request_content` | ✅ | Names differ by convention (Java camelCase, Rust snake_case). Behaviour identical. |
| `logging.displayResponseContent` | `logging.display_response_content` | ✅ | Ditto. |
| `logging.printStackTrace` | `logging.print_stack_trace` | ✅ | Java printed Java exceptions; Rust prints `source()` chain bounded to 5 hops. |
| `logging.meaningfulErrors` | `logging.meaningful_errors` | ✅ | Java emitted an extra ERROR line; Rust emits a WARN (level distinction lets alerting differentiate primary vs. supplementary). |

Additional Rust-side knobs with no Java equivalent:

- `logging.format: text|json`
- `logging.access_log`
- `logging.step_timing`
- `logging.max_body_bytes`
- `logging.redact_headers` (Java had no framework-level redaction)
- `logging.redact_body_fields` (ditto)

## Output format translation

Java Ruuter emitted two shapes via Logback:

1. **Console** — the `C_LOG_PATTERN` from
   `src/main/resources/logback-spring.xml`: a text line with
   `[traceId,spanId]`, thread name, logger name, `[stepType]`,
   `requestTo`, `requestContent`, `responseContent`, `responseCode`,
   `responseInMs`, message.
2. **Rolling file** (`F_LOG_PATTERN`) — TSV with `timeStamp`,
   `version`, `LEVEL`, `requestAuthorIp`, `stepType`,
   `[traceId,spanId]`, `requestTo`, `requestContent`,
   `responseContent`, `responseCode`, `responseInMs`, `message`,
   compressed and rotated (10 MB × 7 days).

Rust replaces both with a single output layer per process:

- **Text format** replaces the Java console pattern for local
  dev.
- **JSON format** replaces the TSV file pattern for production
  ingest. JSON is field-indexed at ingest time and does not
  need the Java-side rotation configuration (containers roll
  stderr via the runtime's log driver).

Java's separate `OpenSearchSender` audit HTTP POST (see
`src/main/java/ee/buerokratt/ruuter/service/OpenSearchSender.java`)
is replaced by the OTLP-log path via the OTel Collector. Same
destination cluster, cleaner ingest.

## Log-level tuning translation

Java Ruuter tuned levels via Spring Boot's `application.yml`
and Logback's per-package loggers:

```yaml
logging:
  level:
    root: INFO
    ee.buerokratt.ruuter.service.DslService: DEBUG
```

Rust tunes via the `RUST_LOG` env var, parsed by
`tracing_subscriber::EnvFilter`:

```bash
RUST_LOG=info,ruuter_on_rust::steps::engine=debug
```

The syntax is different but the model is the same: default
level plus per-module overrides.

## What Ruuter-on-Rust DOESN'T port from Java

Deliberate. Each of these has a modern replacement.

- **In-process rolling file appender**. Container runtimes
  handle rotation via their log driver; systemd services pipe
  to logrotate. See
  [Recipes / non-recipe](./recipes.md#non-recipe--in-process-file-rotation).
- **Micrometer metrics**. Replaced by OpenTelemetry span exports
  (`OTEL_EXPORTER_OTLP_ENDPOINT`).
- **Bespoke `OpenSearchSender` audit sink**. Replaced by OTLP-log
  export via the OTel Collector. Same target cluster, cleaner
  ingest pipeline.
- **Spring Boot's `spring.application.name` for service naming**.
  Replaced by `OTEL_SERVICE_NAME` env var.
- **Coloured console output**. `tracing`'s fmt layer supports
  it but the framework doesn't opt in — container logs and log
  aggregators handle colours themselves.

## Migration checklist

1. **Point OTel Collector at existing OpenSearch cluster.** The
   collector's `elasticsearchexporter` writes into the same
   indices Java's `OpenSearchSender` was populating.
2. **Rewrite dashboards to new field names.** The MDC field
   translation table above is the map.
3. **Rewrite `RUST_LOG` from Logback per-package logger
   configs.** Same model, different syntax.
4. **Delete rolling-file config from your deployment.** Not
   needed under containers; under systemd, add
   `StandardOutput=file:/var/log/ruuter.log` and hand it to
   logrotate.
5. **Set `logging.format: json` and `RUUTER_LOG_FORMAT=json`**
   in prod so ingest doesn't need regex parsing.
6. **Turn on the two content flags for one week** and verify
   redaction covers your project-specific PII / secrets fields
   (see [Redaction / Testing](./redaction.md#testing-your-redaction)).

## Cross-links

- [Field vocabulary](./fields.md) — full field reference on
  the Rust side.
- [Configuration reference](./configuration.md) — every Rust
  knob.
- [Recipes](./recipes.md) — production / dev / hardening
  configs.
- Upstream Java Ruuter source of truth:
  <https://github.com/buerokratt/Ruuter>.

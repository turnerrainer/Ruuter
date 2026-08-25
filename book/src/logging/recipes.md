# Recipes

Ready-to-copy configuration for common scenarios.

## Local dev — human-readable

Default. Just `docker compose up` or `cargo run --bin ruuter-on-rust`.
No `ruuter.yaml` needed.

```bash
cargo run --bin ruuter-on-rust
# → text-format INFO on stderr with access log
```

To make it noisier:

```bash
RUST_LOG=ruuter_on_rust=debug cargo run --bin ruuter-on-rust
```

## Production — JSON to Loki / CloudWatch / Datadog

Set the env var at deploy time:

```bash
docker run -e RUST_LOG=info -e RUUTER_LOG_FORMAT=json \
  turnerrainer/ruuter:latest
```

Or via `docker-compose.yml`:

```yaml
services:
  ruuter:
    image: turnerrainer/ruuter:latest
    environment:
      RUST_LOG: info
      RUUTER_LOG_FORMAT: json
```

Or via Kubernetes:

```yaml
spec:
  containers:
    - name: ruuter
      image: turnerrainer/ruuter:latest
      env:
        - name: RUST_LOG
          value: info
        - name: RUUTER_LOG_FORMAT
          value: json
```

Loki's Docker driver ingests stderr JSON directly, keying on
every field including `trace_id` and `dsl.project`. Grafana
dashboards filter by `{dsl.project="samples", http.response.status_code=~"5.."}`.

CloudWatch, Datadog Log Management, and OpenSearch all similarly
auto-index every field in the JSON object without per-service
config.

## Production — with OTel span export

```bash
docker run \
  -e RUST_LOG=info \
  -e RUUTER_LOG_FORMAT=json \
  -e OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317 \
  -e OTEL_SERVICE_NAME=ruuter-billing \
  turnerrainer/ruuter:latest
```

Now every request produces:

- One JSON access log line to stderr (goes to Loki / CloudWatch).
- One OTLP span exported to the collector (goes to Tempo /
  Jaeger / Datadog APM / Honeycomb).

Both share the same `trace_id` so an operator jumps from a
slow span to the raw log lines via the same id.

## Investigating a specific request

```bash
# Client-side: capture the trace_id from the response
$ curl -sSD - http://your-service/api/thing | grep -i trace
x-trace-id: 4bf92f3577b34da6a3ce929d0e0e4736

# Server-side: filter logs by that id
$ kubectl logs -f deploy/ruuter | jq -c 'select(.fields.trace_id == "4bf92f3577b34da6a3ce929d0e0e4736")'
```

Every event inside that request — access log, step DEBUG lines
(if `step_timing` is on), any DSL `log:` fires, any errors —
comes back stamped with that id.

For a trace initiated by the client:

```bash
$ TP="00-$(openssl rand -hex 16)-$(openssl rand -hex 8)-01"
$ curl -H "Traceparent: $TP" http://your-service/api/thing
$ TID=$(echo "$TP" | cut -d- -f2)
$ kubectl logs deploy/ruuter | jq -c ". | select(.fields.trace_id == \"$TID\")"
```

## Debugging one flaky DSL

Turn on step timing and both content flags; narrow `RUST_LOG` so
the noise stays scoped:

```yaml
# ruuter.yaml (or via env for the container)
logging:
  format: json
  step_timing: true
  display_request_content: true
  display_response_content: true
  meaningful_errors: true
  print_stack_trace: true
```

```bash
RUST_LOG=ruuter_on_rust::steps=debug,ruuter_on_rust=info
```

Then filter by `dsl.project` + step name:

```bash
docker compose logs ruuter | jq -c '. |
  select(.fields."dsl.project" == "checkout") |
  select(.fields."dsl.step" == "fetch_inventory")'
```

Turn back off after the postmortem — high-QPS deployments will
drown in per-step lines if this stays on.

## Hardening — extend redaction

Project-specific secrets should be added to the redact lists.
Defaults are NOT preserved when you supply the field, so re-list
the defaults you want to keep:

```yaml
logging:
  redact_headers:
    - authorization
    - proxy-authorization
    - cookie
    - set-cookie
    - x-api-key
    - x-auth-token
    - x-buerostack-session   # project-specific
    - x-tenant-token         # project-specific

  redact_body_fields:
    - password
    - pass
    - secret
    - token
    - access_token
    - refresh_token
    - api_key
    - authorization
    - ssn                    # PII
    - dob                    # PII
    - medical_record         # PII
    - iban                   # financial
```

Verify by hitting an endpoint that carries the field and
grepping the log for the raw value — it should never appear.
See [Redaction / Testing your redaction](./redaction.md#testing-your-redaction).

## Alerting — 5xx spike detection

With JSON output and Loki, one LogQL query gives you the ratio:

```logql
sum(rate({service="ruuter"} | json | http_response_status_code=~"5.." [5m]))
  /
sum(rate({service="ruuter"} | json | http_response_status_code!="" [5m]))
```

Alert when >1% for 5 minutes. Access log carries the status code
on every request, so no additional instrumentation needed.

## Alerting — slow-request detection

```logql
histogram_quantile(0.95,
  sum by (le) (rate({service="ruuter"} | json | unwrap duration_ms [5m])))
```

Or filter to a specific route:

```logql
histogram_quantile(0.95,
  sum by (le) (
    rate({service="ruuter"}
      | json
      | http_route="/api/checkout"
      | unwrap duration_ms [5m])))
```

## Dashboard skeleton — Ruuter service overview

Suggested panels:

1. **Request rate** — `sum by (dsl.project) (rate({service="ruuter"} | json | http_route!="" [1m]))`
2. **Error rate** — `sum by (http.response.status_code) (rate({service="ruuter"} | json | http_response_status_code=~"[45].." [1m]))`
3. **p95 latency** — `histogram_quantile(0.95, sum by (le) (rate({service="ruuter"} | json | unwrap duration_ms [5m])))`
4. **Access log stream** — `{service="ruuter"} | json | line_format "{{.http_request_method}} {{.http_route}} → {{.http_response_status_code}} ({{.duration_ms}}ms)"`

Every panel keys on the same access-log line — no additional
Ruuter config beyond `logging.format: json` and `access_log: true`
(the default).

## Migration — from Java Ruuter's Logback + OpenSearch

Two moves:

1. **Point OTel Collector at your existing OpenSearch cluster.**
   The OTel Collector's `elasticsearchexporter` writes into
   the same indices Java's `OpenSearchSender` was populating.
2. **Rewrite Java dashboard queries** to the new field names.
   See [Java Ruuter parity](./java-parity.md) for the field-by-
   field translation table.

The Ruuter side: `logging.format: json`,
`OTEL_EXPORTER_OTLP_ENDPOINT=<collector>`. Done.

## Non-recipe — in-process file rotation

Ruuter does not roll files. Use whatever your platform already
does:

- **Docker**: the log driver you configured.
- **Kubernetes**: kubelet's log rotation (defaults are usually
  fine).
- **systemd**: `StandardOutput=journal` or `StandardOutput=file:...`
  with journald / logrotate handling retention.

Java Ruuter's in-process rolling file (10 MB × 7 days, gzipped)
was a bare-metal concern that container deployments don't need.
For bare-metal Rust deployments, pipe Ruuter's stderr into
logrotate the same way you would for any other systemd service.

## Cross-links

- [Configuration reference](./configuration.md) — every knob.
- [Field vocabulary](./fields.md) — field names for LogQL /
  dashboard queries.
- [Traceparent & OpenTelemetry](../framework/tracing.md) — OTel
  span export details.

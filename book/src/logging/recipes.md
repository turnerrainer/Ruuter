# Recipes

Ready-to-copy configuration for common scenarios.

## Local dev — human-readable

Default. Just `docker compose up` or `cargo run --bin ruuter-on-rust`.
No `ruuter.yaml` needed.

```bash
cargo run --bin ruuter-on-rust
# → compact text-format INFO on stderr with access log + per-step Executed trail
```

Prefer coloured output in the terminal:

```bash
RUUTER_LOG_FORMAT=pretty cargo run --bin ruuter-on-rust
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

Every event inside that request — one `Executed` INFO line per
step (with step-type-specific `attrs`), the access log, any DSL
`log:` fires, any errors — comes back stamped with that id. The
DSL execution sequence is on the trail at default INFO level
(issue #37); no `step_timing` needed. Add
`logging.log_dsl_runs: true` if you also want explicit
`DSL run started` / `DSL run completed` bracket lines with a
`terminated_by` label.

For a trace initiated by the client:

```bash
$ TP="00-$(openssl rand -hex 16)-$(openssl rand -hex 8)-01"
$ curl -H "Traceparent: $TP" http://your-service/api/thing
$ TID=$(echo "$TP" | cut -d- -f2)
$ kubectl logs deploy/ruuter | jq -c ". | select(.fields.trace_id == \"$TID\")"
```

## Debugging one flaky DSL

The default INFO trail (issue #37) already tells you what step
ran, in what order, and what its per-type outcome was — one
`Executed` line per step with an `attrs` field (HTTP URL +
upstream status, switch matched branch, state op/key/hit, return
status, etc.). Start by filtering the default output before
adding verbosity:

```bash
docker compose logs ruuter | jq -c '. |
  select(.fields."dsl.project" == "checkout") |
  select(.fields.message == "Executed")'
```

If you need outbound HTTP request/response bodies too (the
default trail names URL + status but not payload), add the two
content flags. The redundant DEBUG-level `step_timing` is only
worth turning on when you also want DEBUG-level chatter from
other modules already suppressed at INFO:

```yaml
# ruuter.yaml (or via env for the container)
logging:
  format: json
  display_request_content: true    # opt-in: outbound bodies
  display_response_content: true   # opt-in: upstream bodies
  meaningful_errors: true          # opt-in: second WARN with underlying cause
  print_stack_trace: true          # opt-in: cause chain on ERROR line
  # log_step_executions is on by default — leave it as-is
  # step_timing: true              # rarely needed; INFO trail is usually enough
```

```bash
RUST_LOG=ruuter_on_rust=info
```

Filter by `dsl.project` + step name to focus:

```bash
docker compose logs ruuter | jq -c '. |
  select(.fields."dsl.project" == "checkout") |
  select(.fields."dsl.step" == "fetch_inventory")'
```

Turn the content flags back off after the postmortem — request
and response bodies stay small only because of `max_body_bytes`,
and shipping full payloads to the log store has cost.

### Silencing the per-step trail on high-QPS DSLs

If a specific deployment measures the per-step INFO lines as too
chatty (rare — one INFO per step at 1k RPS is ~10× the access-log
volume, well inside modern log-store budgets), the trail is
independently toggleable:

```yaml
logging:
  log_step_executions: false   # silences per-step Executed lines
  # log_dsl_runs is already off by default; set true only if you
  # want bracket lines and can afford them.
```

The access log and OTel spans stay on independently — you keep
per-request observability, just lose the intra-request breakdown.

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

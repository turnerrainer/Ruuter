# Recipes

Ready-to-copy configuration for common scenarios.

## Reading a live trail

The per-step INFO trail (issue #37) is easiest to grasp against real
output. Every example below was captured with:

```yaml
logging:
  format: pretty         # ANSI colours + Unicode markers
  log_step_executions: true   # default
  log_dsl_runs: true          # opt-in — brackets the run
```

against the checked-in `DSL/samples/` tree. Rerun any of them via
the imported Postman collection (`postman/ruuter.postman_collection.json`)
or a raw `curl` — every line below is verbatim server output with
ANSI dropped for print.

### `GET /samples/ping` — smallest possible trail

One step. Return only. The four lines are: run start / step / run
end / access log.

```
14:37:50.445 INFO [t=5561e7fc samples] DSL run started  dsl.total_steps=1 dsl.first_step=response
14:37:50.445 INFO [t=5561e7fc samples] ▸ response (return) 36µs → -  status=202 body="pong"
14:37:50.445 INFO [t=5561e7fc samples] DSL run completed  took=196µs dsl.steps_ran=1 terminated_by=return terminating_step=response
14:37:50.445 INFO [t=5561e7fc samples] ⏹ GET /samples/ping 202 598µs  from 127.0.0.1
```

Shared `t=5561e7fc` prefix ties the four lines. For a 1-step DSL
in production, flip `log_dsl_runs: false` — the request span already
frames the run and the brackets triple the volume without new signal.

### `GET /samples/variables/assign-simple` — `assign` + `return`

```
14:39:12.709 INFO [t=c5b65e53 samples] DSL run started  dsl.total_steps=2 dsl.first_step=assign_vars
14:39:12.709 INFO [t=c5b65e53 samples] ▸ assign_vars (assign) 23µs → return_result  keys="age,city,name"
14:39:12.715 INFO [t=c5b65e53 samples] ▸ return_result (return) 5.6ms → -  status=200 body={"user":{"age":30,"city":"Tallinn","name":"John Doe"}}
14:39:12.715 INFO [t=c5b65e53 samples] DSL run completed  took=5.9ms dsl.steps_ran=2 terminated_by=return terminating_step=return_result
14:39:12.715 INFO [t=c5b65e53 samples] ⏹ GET /samples/variables/assign-simple 200 6.4ms  from 127.0.0.1
```

- `keys="age,city,name"` — sorted, comma-joined for deterministic
  grep. Values are not emitted (they can be large or sensitive).
- `body={…}` — return payload preview (redacted per
  `redact_body_fields`, capped at 80 bytes).

### `GET /samples/things` — `switch` with three outcomes

Same DSL, three URL shapes drive three switch branches:

```
▸ route (switch) 5.0ms → list    condition=0 expr="${incoming.params.pathParams.length === 0}"
▸ route (switch) 10.1ms → detail condition=1 expr="${incoming.params.pathParams.length === 1}"
▸ route (switch) 7.7ms → sub     condition=undefined
```

- `condition=<n>` — 0-indexed slot in the DSL's `switch:` list.
- `expr="..."` — raw JS at that slot, so a reader locates the
  branch in the DSL file without opening it.
- `condition=undefined` — no-match case fell through to the
  step-level `next:`. `undefined` (unquoted, JS-native sentinel)
  means a single `condition=` filter catches both matched and
  unmatched runs.

Note the `dsl.total_steps=4` vs `dsl.steps_ran=2` gap in the run
bracket — `total_steps` is what's declared in the DSL, `steps_ran`
is what actually executed. On a switch DSL, that gap is the
value of the brackets.

### `POST /samples/state/inc` — `state.get` + `state.set`

First hit (cold):

```
▸ read_counter  (state)  49µs → bump           op="get" key="counter" hit=false
▸ bump          (assign) 4.8ms → write_counter keys="next_value"
▸ write_counter (state)  3.8ms → respond       op="set" key="counter" value=1
▸ respond       (return) 4.1ms → -             status=200 body={"counter":1}
```

Second hit (warm):

```
▸ read_counter  (state)  40µs → bump           op="get" key="counter" hit=true
▸ bump          (assign) 3.2ms → write_counter keys="next_value"
▸ write_counter (state)  2.9ms → respond       op="set" key="counter" value=2
▸ respond       (return) 3.6ms → -             status=200 body={"counter":2}
```

- `hit=true|false` — appears only on `op=get`; the single most-
  asked question when a DSL reads state and gets an unexpected null.
- `value=<preview>` — appears only on `op=set`; redacted per
  `redact_body_fields` and capped so the log line answers "what did
  I actually store?" without a second trip to the store.

### `GET /samples/advanced/logging-demo?userId=42` — `log` + `assign` + `return`

The `log:` DSL step surfaces its payload as `attrs.msg` on the
same `Executed` line — no separate event. Renders the interpolated
message alongside the step timing.

```
▸ log_start     (log)    25.1ms → process         msg="Request processing started for user: 42"
▸ process       (assign) 6.8ms  → log_processing  keys="timestamp,user_id"
▸ log_processing (log)   5.6ms  → complete        msg="Processing user ID: 42 at 1788187016293"
▸ complete      (assign) 4.2ms  → log_complete    keys="result"
▸ log_complete  (log)    44µs   → respond         msg="Request processing completed successfully"
▸ respond       (return) 5.4ms  → -               status=200 body={"processedAt":1788187016293,"status":"completed","userId":42}
```

CR/LF are stripped from `msg` before emission (log-injection
defence). Message is capped at 256 bytes with a `…` marker if
longer.

### `POST /samples/advanced/iterate-batch` — `iterate`

Body: `{"orders":[{"id":"a","qty":2,"price":10},{"id":"b","qty":3,"price":5}]}`

```
▸ setup (assign)  110µs → work   keys="orders"
▸ work  (iterate) 8.1ms → reply  count=2 as=order
▸ reply (return)  4.7ms → -      status=200 body={"count":2,"totals":[{"id":"a","net":20},{"id":"b","net":15}]}
```

- `count=2` — number of iterations actually run.
- `as=order` — binding name from the DSL, so a reader knows which
  variable each iteration wrote into.
- Inner `do:` steps are NOT emitted per iteration (would explode
  the log). Use `step_timing: true` at DEBUG if you need per-
  iteration timing.

### Where the fields come from

Every `attrs=…` field name maps 1:1 to a
[Configuration reference § `log_step_executions`](./configuration.md#log_step_executions)
entry. The short names inside `attrs` are deliberately terse — the
step type is already on the line (`(state)`, `(switch)`), so
prefixing every field with its type would duplicate. Full OTel
semantic-convention names still appear on the access-log line and
on the OTel span for dashboard portability — see
[Field vocabulary](./fields.md).

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

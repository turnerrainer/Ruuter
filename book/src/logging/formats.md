# Output formats

Two output layers, one at a time (chosen at boot).

## Text (default)

`tracing`'s human-friendly line renderer. Great for `docker logs`,
`journalctl`, and grepping. This is the default because most
first-run experiences of Ruuter are local dev and container-log
tailing, where readability beats machine-parseability.

Example line (line-wrapped here for reading; real output is one
line per event):

```
2026-08-25T10:04:37.180777Z INFO http_request{
  otel.name=HTTP GET /samples/ping
  http.request.method=GET
  http.route=/samples/ping
  dsl.project=samples
  client.address=127.0.0.1
  trace_id=c80300fe7cbd1b35cf1a3e741c82a6ab
}:
  ruuter_on_rust::router: http request completed
  http.request.method=GET
  http.route=/samples/ping
  http.response.status_code=202
  duration_ms=0.95583
  dsl.project=samples
  client.address=127.0.0.1
  trace_id=c80300fe7cbd1b35cf1a3e741c82a6ab
```

The `http_request{...}` prefix is the request-scoped span. Every
line fired inside that span (access log, DSL `log:` step outputs,
step-timing DEBUG lines, errors) inherits the span's fields.

## JSON

One JSON object per event, OTel-log-shape. This is the recommended
production format: Loki / Elastic / CloudWatch / Datadog / OpenSearch
all key on structured fields without regex parsing. Turn on with
`logging.format: json` in config, or `RUUTER_LOG_FORMAT=json` in
env.

```json
{
  "timestamp": "2026-08-25T10:07:12.020926Z",
  "level": "INFO",
  "target": "ruuter_on_rust::router",
  "fields": {
    "message": "http request completed",
    "http.request.method": "GET",
    "http.route": "/samples/basic/hello",
    "http.response.status_code": 200,
    "duration_ms": 1.762463,
    "dsl.project": "samples",
    "client.address": "127.0.0.1",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"
  },
  "span": {
    "name": "http_request",
    "otel.name": "HTTP GET /samples/basic/hello",
    "http.request.method": "GET",
    "http.route": "/samples/basic/hello",
    "dsl.project": "samples",
    "client.address": "127.0.0.1",
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736"
  }
}
```

- **`timestamp`** — ISO-8601 with UTC offset.
- **`level`** — `INFO`, `WARN`, `ERROR`, `DEBUG`, `TRACE`.
- **`target`** — Rust module path emitting the event
  (`ruuter_on_rust::router`, `ruuter_on_rust::steps::engine`, …).
  Useful for `RUST_LOG` scoping.
- **`fields`** — per-event structured fields plus the free-text
  `message`.
- **`span`** — the enclosing request span (if any), with all its
  fields. `span.name` is `http_request` for request-scoped events.

## Selection priority

The format is selected in this priority order:

1. **`RUUTER_LOG_FORMAT=text|json`** env var — highest. Operator
   flips a running container without touching the config file.
2. **`logging.format`** in `ruuter.yaml`.
3. **Default: `text`**.

Any value other than `text` or `json` in the env var is ignored
(config value wins in that case).

## Why not both?

`tracing_subscriber` only ships one fmt layer per process; multiple
format outputs would need a separate downstream (Vector, fluent-bit,
OTel Collector). The simplest deployment shape is:

- **stderr → JSON** in production (Ruuter emits JSON; container
  runtime ships stderr to Loki/CloudWatch/etc.).
- **stderr → text** in local dev (default; nothing to configure).

If you need the same data in two shapes (e.g. Slack alerts + long
retention), do the fan-out at ingest, not in Ruuter.

## Choosing a format

| Situation | Format | Why |
|---|---|---|
| Local dev, terminal tailing | `text` | Human-readable, colorised by the terminal |
| CI test logs | `text` | Fits the CI runner's log viewer |
| Docker Compose stack, dev | `text` | `docker compose logs` is grep-friendly |
| Kubernetes / Cloud Run / any log aggregator | `json` | Field-based indexing beats regex |
| Loki, Elastic, OpenSearch, Datadog, CloudWatch, Splunk | `json` | Native structured ingest |
| Bare-metal with logrotate / journalctl | either | Both roll fine; text is easier to grep in one file, JSON is easier once you pipe through `jq` |
| Ephemeral one-shot debug | `text` | You are the reader |

The env-var override means you can leave `logging.format: text` in
the checked-in `ruuter.yaml` for local dev and flip to JSON in the
production Kubernetes deployment by setting one env var on the
container spec.

## Interaction with `RUST_LOG`

`RUST_LOG` (parsed by `tracing_subscriber::EnvFilter`) filters
what levels are emitted at all. Format is what the emitted lines
look like. Common recipes:

```bash
RUST_LOG=info                                          # default — access log + boot + DSL log:
RUST_LOG=ruuter_on_rust=debug,reqwest=warn             # step_timing + outbound bodies, upstream chatter quiet
RUST_LOG=warn                                          # only warnings + errors
RUST_LOG=ruuter_on_rust::steps::engine=debug,info      # engine DEBUG, everything else INFO
```

## Cross-links

- [Field vocabulary](./fields.md) — which fields appear in which
  format.
- [Configuration reference](./configuration.md) — the
  `logging.format` knob.
- [Environment variables](../ops/env.md) — `RUUTER_LOG_FORMAT`,
  `RUST_LOG`.

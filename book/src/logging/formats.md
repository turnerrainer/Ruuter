# Output formats

Three output layers, one at a time (chosen at boot).

## Text (default)

Compact one-line-per-event terminal-first layout. Each line looks
like this — no wrapping on any terminal ≥ 120 columns:

```
10:34:59.099 INFO  [t=2b380de4 samples] ▸ read_counter (state) 49µs → bump  op="get" key="counter" hit=false
10:34:59.103 INFO  [t=2b380de4 samples] ▸ bump (assign) 4.8ms → write_counter  keys="next_value"
10:34:59.107 INFO  [t=2b380de4 samples] ▸ write_counter (state) 3.8ms → respond  op="set" key="counter" value=1
10:34:59.112 INFO  [t=2b380de4 samples] ▸ respond (return) 4.1ms → -  status=200 body={"counter":1}
10:34:59.112 INFO  [t=2b380de4 samples] ⏹ POST /samples/state/inc 200 13.6ms  from 127.0.0.1
```

Anatomy of a `▸` line (per-step `Executed`):

```
HH:MM:SS.mmm LEVEL [t=<8hex trace_id> <project>] ▸ <step-name> (<step-type>) <duration> → <next-step>  <attrs>
```

Anatomy of a `⏹` line (access log):

```
HH:MM:SS.mmm LEVEL [t=<8hex trace_id> <project>] ⏹ <METHOD> <route> <status> <duration>  from <client-ip>
```

Design choices behind the format:

- **8-hex short trace_id** in the `[t=… project]` prefix — enough
  to disambiguate concurrent requests locally, greppable, doesn't
  eat the line. The full 32-hex value stays on the OTLP span and
  in the `X-Trace-Id` response header for cross-service tracing.
- **Duration in the closest unit** — `49µs` / `4.8ms` / `1.23s`
  instead of `0.049` / `4.8` / `1230.0` with an implicit `ms` unit.
- **Rust module target dropped** — `ruuter_on_rust::steps::engine`
  is noise to a DSL reader.
- **OTel semantic-convention span fields elided** — `otel.name`,
  `http.request.method`, `http.route`, `client.address` still ride
  on the span for OTLP export, but text output filters them out
  because they duplicate what's on the access-log line.

With `logging.log_dsl_runs: true` set, two additional INFO lines
frame every run: `DSL run started` before the first `Executed`
line and `DSL run completed` after the last, carrying
`terminated_by=return|end_of_steps|iteration_cap|error` and the
step-count / total-duration summary. Off by default because the
request span already brackets each run via `trace_id`.

## Pretty (opt-in, local dev)

Same layout as `text` plus ANSI colours and Unicode markers.
Enable with `logging.format: pretty` or `RUUTER_LOG_FORMAT=pretty`.

- Timestamp and `[t=… project]` prefix: dim grey
- Level: green (INFO), yellow (WARN), red (ERROR)
- `▸` (step marker): cyan
- Step name: bold
- Step type: dim
- Duration: cyan
- `→` (next arrow): dim
- `⏹` (access-log marker): magenta
- Status: green (2xx), yellow (3xx), red (4xx/5xx)

Only turn on when the terminal will render ANSI. Piping to a file
or a log aggregator leaks colour escapes — use `text` or `json`
for those.

The `attrs=…` field on each `Executed` line carries step-type-
specific context that Java Ruuter's polymorphic `logStep` emitted
into MDC (issue #37). For a `switch`, it's `condition=<n>` (matched
slot) or `condition=undefined` (no-match) + `expr=…`; for `http`,
the URL + upstream status; for `state`, the op + key + hit; and so
on. See the [Configuration reference](./configuration.md#log_step_executions)
for the full per-type vocabulary, or [Recipes → Reading a live
trail](./recipes.md#reading-a-live-trail) for annotated end-to-end
examples per step type.

## JSON

One JSON object per event, OTel-log-shape. This is the recommended
production format: Loki / Elastic / CloudWatch / Datadog / OpenSearch
all key on structured fields without regex parsing. Turn on with
`logging.format: json` in config, or `RUUTER_LOG_FORMAT=json` in
env.

A single request in the default config produces one JSON object
per event. For `GET /samples/basic/hello`, the two-line trail looks
like this (line-wrapped here for reading; each object is one line
in real output). The `span` block is identical across both and is
elided on the second for brevity:

```json
{
  "timestamp": "2026-08-25T10:07:12.019Z",
  "level": "INFO",
  "target": "ruuter_on_rust::steps::engine",
  "fields": {
    "message": "Executed",
    "dsl.step": "response",
    "dsl.step.type": "return",
    "duration_ms": 0.032,
    "dsl.next.step": "-",
    "attrs": "status=200 body=\"hello\""
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
{
  "timestamp": "2026-08-25T10:07:12.021Z", "level": "INFO",
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
  "span": { … }
}
```

With `logging.log_dsl_runs: true`, two additional `DSL run started`
and `DSL run completed` objects bracket the `Executed` line —
useful when you want an explicit `terminated_by` label in the
stream, redundant when the request span is already giving you
per-request framing.

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

1. **`RUUTER_LOG_FORMAT=text|pretty|json`** env var — highest.
   Operator flips a running container without touching the config
   file.
2. **`logging.format`** in `ruuter.yaml`.
3. **Default: `text`**.

Any value other than `text` / `pretty` / `json` in the env var is
ignored (config value wins in that case).

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
| Local dev, interactive terminal | `pretty` | ANSI colours + Unicode markers, easiest to scan |
| Local dev, tailing a file | `text` | Same compact layout, no colour escapes |
| CI test logs | `text` | Fits the CI runner's log viewer; no colour |
| Docker Compose stack, dev | `text` | `docker compose logs` is grep-friendly |
| Kubernetes / Cloud Run / any log aggregator | `json` | Field-based indexing beats regex |
| Loki, Elastic, OpenSearch, Datadog, CloudWatch, Splunk | `json` | Native structured ingest |
| Bare-metal with logrotate / journalctl | `text` or `json` | `text` grepable in one file, `json` easier once piped through `jq` |
| Ephemeral one-shot debug | `pretty` | You are the reader |

The env-var override means you can leave `logging.format: text` in
the checked-in `ruuter.yaml` for local dev and flip to JSON in the
production Kubernetes deployment by setting one env var on the
container spec.

## Interaction with `RUST_LOG`

`RUST_LOG` (parsed by `tracing_subscriber::EnvFilter`) filters
what levels are emitted at all. Format is what the emitted lines
look like. Common recipes:

```bash
RUST_LOG=info                                          # default — boot + access log + per-step Executed + DSL log:
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

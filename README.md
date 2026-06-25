# Ruuter-RS

**Rust implementation of Ruuter - Declarative REST Router**

Version: 0.4.0
Author: Rainer Türner
Status: Functional Core Complete

## Features

- ✅ File-system-based REST routing
- ✅ YAML DSL parser
- ✅ JavaScript expression evaluation (Boa engine)
- ✅ HTTP client (GET/POST/PUT/DELETE)
- ✅ All core step types (assign, return, http, switch, log, state, iterate, ws_send)
- ✅ Constants.ini support
- ✅ Configuration system
- ✅ Error handling
- ✅ Docker support
- ✅ WebSocket sources (consume upstream feeds → trigger DSLs)
- ✅ WebSocket server (accept inbound WS clients → run WS DSLs per frame)
- ✅ `ws_send` step (reply to caller / fan-out / send upstream)
- ✅ Guards (`*.guard.yml` per-directory)
- ⚠️ Template step (basic)

## Quick Start

Docker is the supported workflow; the local `cargo` path is for IDE
type-checking only.

```bash
docker compose up -d --build
```

Server starts on `http://localhost:8080`. Health check at
`GET /health`.

## Example HTTP DSL

```yaml
# DSL/samples/GET/ping.yml
response:
  status: 202
  return: pong
```

Access: `GET http://localhost:8080/samples/ping`

## WebSocket server

Drop a DSL at `DSL/<project>/WS/<path>.yml` and clients can connect
to `ws://localhost:8080/<project>/<path>`. The DSL is invoked once
per inbound frame. Inside the DSL:

- `incoming.body` — parsed JSON of the inbound frame (or
  `incoming.body.value` for non-JSON text)
- `incoming.connection_id` — per-client id (namespace `client:<hex>`)
- `incoming.headers` / `incoming.params` — handshake headers and
  query string (snapshotted at upgrade, identical across frames)

Reply to the originating client:

```yaml
reply:
  ws_send:
    payload: { type: "echo", got: "${incoming.body}" }
```

Fan-out to every connected client:

```yaml
fanout:
  ws_send:
    broadcast_prefix: "client:"
    payload: { from: "${incoming.connection_id}", msg: "${incoming.body}" }
```

Send to a specific connection id (resolved from a script expression):

```yaml
direct:
  ws_send:
    to: "${target_cid}"
    payload: { type: "dm", text: "${incoming.body.text}" }
```

Worked samples: `DSL/samples/WS/{echo,broadcast,chat}.yml`.

## WebSocket sources (consume upstream feeds)

Configure an outbound WS source at `DSL/<project>/sources/<name>.yml`:

```yaml
kind: websocket
url: "wss://stream.data.alpaca.markets/v2/iex"
on_connect:
  - send_json: { action: auth, key: "[#alpaca_api_key]", secret: "[#alpaca_api_secret]" }
  - send_json: { action: subscribe, bars: ["AAPL", "MSFT", "..."] }
dispatch:
  channel: "$.T"   # dot-path → trigger channel
  key:     "$.S"   # dot-path → trigger key
```

Each inbound frame dispatches to
`DSL/<project>/triggers/<channel>/<key>.yml` (with `_default.yml` as
fallback). The same `StepEngine` runs the DSL — every step type
(`assign`, `state`, `http`, `switch`, `ws_send`, …) is available, so
ONE `_default.yml` can serve hundreds of symbols. The source's own
outbound sink is registered as `source:<project>:<name>` so a trigger
DSL can `ws_send` back upstream (e.g. mid-stream subscription updates).

Worked stock-monitoring sample (criteria-gated POST alert):
`DSL/samples/triggers/stock-bars/_default.yml` with a per-symbol
override at `AAPL.yml` and the source template at
`DSL/samples/sources/stock-feed.yml.disabled`.

## Docker Configuration

The Docker image uses multi-stage builds for minimal size:
- Build stage: Rust 1.75
- Runtime stage: Debian slim
- Non-root user for security
- Volume mounts for DSL files and constants

### Volumes

- `./DSL:/app/DSL:ro` - DSL files (read-only)
- `./constants.ini:/app/constants.ini:ro` - Constants (read-only)

### Environment

- `RUST_LOG=info` - Logging level (debug, info, warn, error)

## Documentation

- [Development TODO](docs/todo.md)
- [CHANGELOG.md](CHANGELOG.md)

## Buerostack integration

Ruuter is one component of the broader Buerostack architecture. It owns:

- HTTP request routing and reverse-proxy duties
- WebSocket server endpoints (`DSL/<project>/WS/<path>.yml`) with
  per-connection identity and `ws_send` for replies / fan-out
- Event-trigger dispatch for non-HTTP push sources (WebSocket today)
- Ephemeral, in-process state for hot signal-processing work
- Pre-execution guards (`*.guard.yml`) for auth / allowlist / validation

It does NOT own:

- **Scheduled work** — that's [CronManager](../CronManager/). See below.
- **Persistent storage** — that's [Resql](../Resql/), SQL files → REST.
- **Identity / JWT issuance** — that's [TIM](../TIM/).
- **Payload shaping between services** — that's [DataMapper](../DataMapper/).

### Scheduled jobs (CronManager → Ruuter)

Schedules live in CronManager, which fires HTTP requests on cron expressions.
Ruuter exposes the work as a normal HTTP DSL.

1. Define the scheduled endpoint in Ruuter:
   `DSL/<project>/POST/scheduled/<job>.yml`
2. Define the job in CronManager:
   ```yaml
   my_job:
     trigger: "0 */5 * * * ?"   # every 5 minutes
     type: http
     method: POST
     url: http://ruuter:8080/<project>/scheduled/<job>
   ```
3. (Production) protect the endpoint with a guard verifying a shared secret
   so external callers cannot trigger it:
   `DSL/<project>/POST/scheduled.guard.yml`

A worked sample lives in `DSL/samples/POST/scheduled/heartbeat.yml`
with the companion CronManager job at
`DSL/samples/cronmanager-jobs/heartbeat.yaml`.

### Observability

OpenTelemetry tracing is wired but inactive by default. To enable:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317 \
OTEL_SERVICE_NAME=ruuter-rs \
docker-compose up
```

W3C TraceContext (`traceparent` / `tracestate`) propagation is configured
to match the rest of the Buerostack components.

### Admin endpoint

`GET /_/sources` returns the source supervisor's view of every event
source's health (status, restart count, last error). Off by default —
enable with `RUUTER_ADMIN_ENABLED=true`.

## Original Project

Rust rewrite of: https://github.com/buerokratt/Ruuter

## License

MIT License

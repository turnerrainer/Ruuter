# Ruuter-RS

Rust implementation of Ruuter — a declarative REST/WebSocket router
driven by YAML DSLs on disk.

**Version:** 0.4.0 · **License:** MIT · **Author:** Rainer Türner

## Set up from scratch

Prerequisites: Docker + Docker Compose.

```bash
git clone <this-repo> ruuter-rs
cd ruuter-rs
docker compose up -d --build
```

- Serves on `http://localhost:8080`.
- Health check: `curl http://localhost:8080/health` → `{"status":"ok",...}`.
- Sample route: `curl http://localhost:8080/samples/ping` → `"pong"`.
- OpenAPI spec (auto-generated from every DSL): `curl http://localhost:8080/_/openapi.json`.

To wipe and rebuild after code changes:

```bash
docker compose down
docker compose up -d --build --force-recreate
```

## How it works

A DSL file at `DSL/<project>/<METHOD>/<path>.yml` becomes the route
`<METHOD> /<project>/<path>`. Example:

```yaml
# DSL/samples/GET/ping.yml
response:
  status: 202
  return: pong
```

Reachable at `GET /samples/ping`.

### Step types

`assign`, `return`, `http` (`http.get`/`post`/`put`/`patch`/`delete`),
`switch`, `log`, `state`, `iterate`, `ws_send`, `template`.
See `DSL/samples/README.md` for worked examples of each.

### Guards

`<stem>.guard.yml` next to a directory protects every DSL under it.
A guard returning `status >= 400` short-circuits the request.

### WebSocket server

Drop `DSL/<project>/WS/<path>.yml` and clients connect at
`ws://localhost:8080/<project>/<path>`. The DSL runs once per inbound
frame with `incoming.body`, `incoming.connection_id`, `incoming.headers`,
`incoming.params`. Reply via `ws_send`.

### WebSocket sources (consume upstream)

Configure at `DSL/<project>/sources/<name>.yml`; each inbound frame
dispatches to `DSL/<project>/triggers/<channel>/<key>.yml` (with
`_default.yml` as fallback). See
`DSL/samples/sources/stock-feed.yml.disabled`.

## Configuration

Layout:

- `DSL/` — routes/guards/triggers/sources/WS DSLs (mounted read-only).
- `constants.ini` — `[#KEY]` values referenced from DSLs (mounted RO).
- `ruuter.yaml` — operator config file (optional, see below).
- `docker-compose.yml` — deployment; container is hardened
  (`read_only`, `no-new-privileges`, `cap_drop: ALL`, mem/cpu limits).
- Environment: `RUST_LOG=info|debug|warn|error`.

### Constants and secrets

DSLs reference `[#KEY]` values from a `constants.ini` file mounted
into the container (read-only). Section headers (`[DSL]`, etc.) are
accepted for Java-Ruuter compatibility but do not scope keys — every
`KEY=value` line is flat. Comments start with `#`. Missing keys
referenced from a WS source config error at load time; missing keys
in a DSL body are substituted as literal `[#KEY]` (visible at runtime).

**Secrets management is out of scope.** Ruuter reads constants from a
file — it does NOT fetch from Vault, KMS, Docker secrets, or any
external store. Mount the resolved secrets file at
`/app/constants.ini` (or bind a Vault-agent-rendered file over it).
Rotation, sourcing, and access control are the deployment pipeline's
job, not the framework's.

### Config file resolution

At boot Ruuter looks for a YAML config file in this priority:

1. `--config <path>` CLI flag.
2. `RUUTER_CONFIG=<path>` env var.
3. `./ruuter.yaml` or `./ruuter.yml` in the working directory.
4. Built-in defaults if none of the above exists.

A worked example with every top-level knob lives at
`DSL/samples/ruuter.yaml.example` — copy to `./ruuter.yaml` and edit.

The full config surface (CORS, CSRF Origin allow-list, Idempotency-Key
cache, SSRF allow-list, response-size cap, method allow-list, Boa
runtime limits, etc.) is documented in `src/config/mod.rs`. Every
setting has a safe default; only override what you need.

## Observability

OpenTelemetry OTLP export is opt-in:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317 \
OTEL_SERVICE_NAME=ruuter-rs \
docker compose up -d --build
```

W3C traceparent is adopted or generated on every request and echoed
back with `X-Trace-Id`; outbound HTTP calls forward it automatically.

## Admin endpoint

`GET /_/sources` reports the source supervisor's health. Off by
default; enable with `RUUTER_ADMIN_ENABLED=true`.

## Buerostack integration

Ruuter owns HTTP routing, WebSocket endpoints, event-trigger dispatch,
ephemeral in-process state, and pre-execution guards. It does **not**
own: scheduled work (CronManager), persistent storage (Resql),
identity/JWT (TIM), inter-service payload shaping (DataMapper).

For CronManager → Ruuter scheduled jobs, define the endpoint in Ruuter
(`DSL/<project>/POST/scheduled/<job>.yml`) and a matching HTTP job in
CronManager. Protect production endpoints with a guard verifying a
shared secret. Worked sample:
`DSL/samples/POST/scheduled/heartbeat.yml` +
`DSL/samples/cronmanager-jobs/heartbeat.yaml`.

## Documentation

- **[Book (mdBook)](./book/src/SUMMARY.md)** — full LLM-oriented reference. Build locally with `mdbook serve book`; browses at http://localhost:3000. Auto-deployed to GitHub Pages on push to `main` (see `.github/workflows/docs.yml`).
- [DSL reference (single page)](docs/DSL_REFERENCE.md) — same content, single Markdown file.
- [CHANGELOG.md](CHANGELOG.md)
- [Development TODO](docs/todo.md)
- Original Java Ruuter: https://github.com/buerokratt/Ruuter

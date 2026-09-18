# Ruuter-on-Rust

Rust implementation of Ruuter — a declarative REST/WebSocket router
driven by YAML DSLs on disk.

**Version:** 0.10.1-rc (pre-release; v1.0.0 is the next stable target) · **License:** Apache-2.0 · **Author:** Rainer Türner

> **Upgrading from v0.10.0-rc?** Patch RC — six h2ck.me v1
> backlog items shipped as one batch (T-23, T-24, T-28, T-30,
> T-31, T-32; PRs #126–#131).
>
> **One client-facing wire behaviour change** for HTTP clients
> uploading multipart:
> - A `multipart/form-data` body carrying more than
>   `incoming_requests.multipart_max_parts` (default `100`) or a
>   single part larger than `incoming_requests.multipart_max_part_size`
>   (default `4 MiB`) now returns `413 Payload Too Large` with a
>   structured JSON body naming the violated limit. Set either
>   cap to `null` in ruuter.yaml to preserve pre-fix unbounded
>   behaviour. Clients that hard-coded "400 == any multipart
>   problem" need a 413 branch.
>
> **Operator-facing surface**: graceful shutdown on SIGTERM /
> SIGINT — inbound requests drain up to 15 s before the process
> exits, so Kubernetes rolling deploys / `docker stop` no longer
> tear in-flight responses. Two new multipart caps
> (`multipart_max_parts`, `multipart_max_part_size`) under
> `incoming_requests`; both default-on with sensible values, both
> opt-out via `null`.
>
> **Under the hood** (concurrency + fuzz surface): `StateStore::set`
> TOCTOU on same-key contention closed via DashMap `Entry` API
> — the per-project entry counter no longer over-reports under
> load. New `fuzz/` crate scaffolding + nightly CI workflow for
> the DSL loader and JSON body deserialiser; scaffolding only, no
> production code touch.
>
> **Docs + tests**: `book/src/dsl/context.md` documents the
> `incoming.params` last-wins semantics for duplicate query
> keys, and fixes a stale table entry that claimed
> `incoming.query` existed (it doesn't — only `incoming.params`
> is bound in the JS runtime). Positive-control regression pin
> for `serde_json`'s implicit ~128-layer JSON depth limit — if a
> future dep bump changes the ceiling, the pin fails loudly.
>
> Full detail in [CHANGELOG.md § 0.10.1-rc](CHANGELOG.md#0101-rc---2026-09-18).

## Try it in one command

Multi-arch image (linux/amd64 + linux/arm64) on Docker Hub and GHCR:

```bash
docker run -d --name ruuter -p 8080:8080 \
    turnerrainer/ruuter:0.10.1-rc
```

- Health check: `curl http://localhost:8080/health` → `{"status":"ok"}`.
- Sample route: `curl http://localhost:8080/samples/ping` → `"pong"`.
- OpenAPI spec (auto-generated from every DSL): `curl http://localhost:8080/_/openapi.json` — admin-gated, requires `RUUTER_ADMIN_ENABLED=true`.

The image bakes in `DSL/samples/` so every endpoint under `/samples/*`
works out of the box. Mount your own tree to override:

```bash
docker run -d --name ruuter -p 8080:8080 \
    -v $(pwd)/DSL:/app/DSL:ro \
    -v $(pwd)/constants.ini:/app/constants.ini:ro \
    turnerrainer/ruuter:0.10.1-rc
```

Prefer a shorter pull recipe? While we're on release candidates,
`:rc` always points at the latest RC — it moves each time a new
`-rc.N` publishes, and it never touches `:latest`:

```bash
docker pull turnerrainer/ruuter:rc
```

Every published digest is signed keyless via cosign — verify with the
recipe in [book/src/ops/docker.md](book/src/ops/docker.md#verify-the-image-cosign).

## Lint / test your DSL tree in CI (issue #83)

`dsl-lint` (static analysis) and `dsl-test` (runtime scenarios) ship
inside the same image as the runtime binary, at exactly the engine
version they'll be validating against:

```bash
# Lint every DSL under ./DSL against constants.ini.
docker run --rm -v "$PWD:/w" -w /w \
    turnerrainer/ruuter:0.10.1-rc \
    dsl-lint --dsl DSL --constants constants.ini

# Run every DSL-test scenario under ./DSL-tests.
docker run --rm -v "$PWD:/w" -w /w \
    turnerrainer/ruuter:0.10.1-rc \
    dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
```

`:rc` for tracking the latest RC in CI; pin to `:0.9.13-rc` in a
release-branch CI so a downstream job doesn't silently upgrade
tooling mid-flight.

## Build from source

For hacking on Ruuter itself:

```bash
git clone -b dev https://github.com/turnerrainer/Ruuter.git ruuter-on-rust
cd ruuter-on-rust
docker compose up -d --build
```

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
OTEL_SERVICE_NAME=ruuter-on-rust \
docker compose up -d --build
```

W3C traceparent is adopted or generated on every request and echoed
back with `X-Trace-Id`; outbound HTTP calls forward it automatically.

## Admin endpoint

`GET /_/sources` reports the source supervisor's health.
`GET /_/unguarded` reports which routes are guarded vs unguarded
(HTTP + WS; issue #45).
`GET /_/openapi.json` returns the auto-generated OpenAPI 3.1 spec.
All three off by default; enable with `RUUTER_ADMIN_ENABLED=true`.

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
- [CLAUDE.md](CLAUDE.md) — brief for coding agents: release-gate commands, breaking-change surface, best-practice config matrix, grep recipes for finding risky settings.
- [Development TODO](docs/todo.md)
- Original Java Ruuter: https://github.com/buerokratt/Ruuter

# Changelog

All notable changes to Ruuter-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0-rc.1] - 2026-07-27

See the [root `CHANGELOG.md`](https://github.com/turnerrainer/Ruuter/blob/dev/CHANGELOG.md#080-rc1---2026-07-27)
for the full entry. Summary: first pre-release cut for partner
testing. Multi-arch container publish + cosign + Trivy + SBOM +
smoke test in CI. DSL hot-reload (opt-in). `#{KEY}` alternate
constant syntax. Getting-started rewrite. Not GA — `main` is
reserved for `v1.0.0`.

## [0.7.0] - 2026-07-24

Security-hardening release. Closes 15 findings from the h2ck.me
pre-publication audit (S1–S8, N1–N4, F1, F2) across three review
rounds; adds a `cargo audit` CI gate. Every fix has a regression
test in `tests/security*.rs` (68 tests total, all green).
`cargo audit --deny warnings` is clean.

Also in this batch (release audit sweep, 2026-07-24):

- `state.delete` accepts `remove:` as a serde alias.
- `dsl-test`'s `mock-http` and `trigger-inject` modes now build the
  test harness with `block_private_networks=false` so DSLs under
  test can reach the in-process mock upstream on 127.0.0.1.
  Production `check_ssrf` behaviour unchanged.
- Repo-wide `cargo fmt` applied; `[lints.clippy]` promoted to a
  hard CI gate with a small, documented allowlist for test-fixture
  patterns.
- Five `v1.0.0` doc references corrected to `v0.7.0`.
- `DSL/samples/POST/idempotent-transfer.yml` header rewritten to
  describe the DSL-authored idempotency pattern.

### Breaking

- **Framework-level `Idempotency-Key` handling removed.** The
  framework no longer caches or replays responses by
  `Idempotency-Key`; the `Idempotency-Replayed` response header is
  never emitted. Two identical POSTs with the same key both execute
  the DSL. DSL authors implement idempotency via `state.get` /
  `state.set` with their own identity + body-hash keys — see
  [Idempotency pattern](../dsl/idempotency-pattern.md). Closes
  h2ck.me findings **S1** (missing body-hash — cross-caller replay)
  and **S5** (`Idempotency-Replayed` oracle). Config field
  `internal_requests.idempotency` and struct `IdempotencyConfig` are
  removed.

### Security

- **S2 — SSRF allowlist exact origin match.** Bare-origin entries in
  `internal_requests.allowed_urls` now require exact
  `scheme://host:port` equality against the request URL. A previous
  substring match let `http://api.example.com` admit
  `http://api.example.com.evil.tld/x`. Path-scoped entries still work
  and prefix-match on the path portion after the origin matches.
- **S3 — `internal_requests.disabled` honoured by every transport.**
  The disabled guard runs at the very top of `HttpClient::request`,
  before self-call short-circuit, `unix://` scheme dispatch, and
  `unix_socket_map` alias dispatch. Disabling outbound HTTP now
  really means every transport.
- **S4 — `X-Forwarded-For` trusted-proxy gating.** New
  `proxy.trusted: [ip, ...]` config. XFF (and `X-Real-IP`) is only
  promoted into `incoming.origin` when the direct TCP peer's IP is
  on the list; otherwise `origin` is the socket peer. Raw header is
  still visible in `incoming.headers`. Empty `proxy.trusted`
  (default) is safe for direct-exposed deployments. New DSL field
  `incoming.origin` is exposed in the scripting scope (Boa +
  QuickJS).

## [0.4.0] - 2026-07-14

### Added
- WebSocket server: DSLs at `DSL/<project>/WS/<path>.yml` run per inbound
  frame with `incoming.connection_id`, `incoming.headers`, `incoming.params`.
- `ws_send` step for replying to caller, fan-out via `broadcast_prefix`, or
  sending to a specific connection id.
- WebSocket sources: outbound feeds at `DSL/<project>/sources/*.yml` dispatch
  each frame to `triggers/<channel>/<key>.yml` under supervisor with
  exponential-backoff reconnect and jitter.
- WS-source upgrade headers so auth-on-upgrade integrations (e.g. Andmela
  `X-Andmela-Token`) can be configured per source.
- Guards primitive (`*.guard.yml`) — per-directory pre-execution DSLs that
  short-circuit on status ≥ 400.
- `iterate` step with `over`, `as`, `do`, `collect`/`into`, `max_items`.
- Source supervisor with `/_/sources` admin endpoint (opt-in via
  `RUUTER_ADMIN_ENABLED=true`).
- `http.patch` DSL step (task 023 — filed after 2026-06-29 live-paper
  outage in stocktrading-dev/desk where broker stops failed to ratchet).
- CORS layer wired from `cors.allowed_origins` / `cors.allow_credentials`.
- Framework-level Idempotency-Key handling (PATTERNS.md §2) with an
  in-process TTL cache; `Idempotency-Replayed: true` on cache hits.
  *(Removed in v0.7.0 — see the [0.7.0] section above.)*
- Origin/Referer CSRF check (PATTERNS.md §1) on state-changing methods.
- W3C traceparent adoption/generation + echo on responses with
  `X-Trace-Id`; outbound http calls auto-forward traceparent.
- `If-Match` framework enforcement (PATTERNS.md §3) — opt-in via
  `optimistic_concurrency.require_if_match`.
- SSRF allow-list on outbound HTTP: `internal_requests.disabled`,
  `allowed_urls` (URL prefix), `allowed_ips` (host-string match).
- `http_response_size_limit` enforcement via streaming.
- `http_codes_allow_list` filter on upstream response status.
- `response_default_headers` merged into every response.
- `incoming_requests.allowed_method_types` enforcement (405 on reject).
- Boa runtime limits: `scripting.max_loop_iterations`,
  `scripting.max_stack_size` protect against runaway JS in DSLs.
- Container hardening: `read_only`, `no-new-privileges`, `cap_drop: ALL`,
  memory/CPU limits, `tini` as init.
- `GET /_/openapi.json` — OpenAPI 3.1 spec auto-generated from the AS-IS
  DSL tree at boot. Every route becomes one operation; response codes
  are inferred from `return.status` literals across the DSL's return
  steps and default to 200 when unresolvable statically. WS/ and
  cronmanager-jobs/ subdirectories are excluded.

### Changed
- Migrated `serde_yaml` (unmaintained per RUSTSEC-2024-0320) to `serde_yml`.
- `HttpClient::new` now takes `&AppConfig` (breaking for external callers —
  use `HttpClient::with_timeout_ms(u64)` for the bare-bones variant).
- `DslRouter::new` takes an explicit `StepEngine` argument so config-driven
  engine limits (`max_step_recursions`) apply to both HTTP routes and
  event triggers.
- Malformed JSON bodies on `Content-Type: application/json` requests now
  return 400 instead of silently coercing to an empty map.
- Deprecated `opentelemetry_sdk::trace::Config` API replaced with
  `TracerProvider::builder().with_resource(...)`.

### Fixed
- Dockerfile healthcheck: `curl` now installed in the runtime stage
  (container was `unhealthy` since 0.3.0 despite `/health` returning 200).
- `VERSION` file bumped to match Cargo.toml (was stuck at 0.3.2).

### Removed
- Dead `src/guards/mod.rs` stub that always returned `Ok(true)`; real
  guard enforcement lives in `router::applicable_guards`.
- `cronmanager-jobs/` under project directories is no longer loaded as
  an HTTP method — added to reserved subdirs alongside `triggers/` and
  `sources/`. Files there are companion CronManager configs, not routes.

## [0.3.2] - 2025-11-03

### Added
- Template step samples demonstrating reusable DSLs
  - user-profile.yml - reusable user fetching template
  - create-entity.yml - entity creation with metadata
  - call-template.yml - example of calling templates
  - call-create-template.yml - template with validation
- Guard samples for authentication/authorization
  - protected.guard.yml - Bearer token authentication
  - admin.guard.yml - Role-based access control
  - protected/data.yml - protected endpoint example
  - admin/delete-user.yml - admin-only endpoint
  - guards-demo.yml - guard explanation and usage
- Updated samples README with:
  - Template syntax and examples
  - Guard documentation and hierarchical structure
  - Guard file naming conventions
  - Usage examples with curl commands

### Documentation
- Added comprehensive template documentation
- Added guard system explanation
- Included hierarchical guard examples
- Updated quick reference with template and guard syntax

## [0.3.1] - 2025-11-03

### Added
- Comprehensive DSL sample library (20+ samples)
- Basic, variables, HTTP, conditionals, JavaScript, and advanced samples
- DSL/samples/README.md with complete documentation

## [0.3.0-docker-support] - 2025-11-03

### Added
- Docker support with multi-stage builds
- docker-compose.yml for easy deployment

## [0.2.0-functional-core] - 2025-11-03

### Added
- Complete DSL parser with YAML support
- File-based routing system
- JavaScript engine integration
- All core step types

## [0.1.0-rust-foundation] - 2025-11-03

### Added
- Initial project structure
- Dependency configuration
- Documentation
- Git workflow

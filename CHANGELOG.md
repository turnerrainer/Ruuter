# Changelog

All notable changes to Ruuter-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.5] - 2026-07-19

### Added

- **Task 036 — per-request QuickJS session pool.** First `evaluate()`
  in a request lazily builds a Runtime + Context pair and caches them
  on `ExecutionContext` via `Arc<OnceLock<QuickJsSession>>`;
  subsequent evaluates in the same request reuse the session. Feature-
  gated to `scripting-quickjs` — Boa's `Context` remains `!Send`, so
  it can't sit on `ExecutionContext` across `.await`. On Boa the field
  simply doesn't exist; behaviour unchanged.

### Perf (compound of tasks 051 + 036)

Measured 3-run median on a developer laptop, `scripting-boa` (default)
vs `--no-default-features --features scripting-quickjs`:

| Scenario | Boa | QuickJS+036 | Δ rps | Δ p50 |
|---|---:|---:|---|---|
| guarded (guard + auth check + main DSL) | 1,401 rps | **6,118 rps** | **+337%** | -78% |
| js-heavy (Boa `Date.now()` + object literal) | 3,245 rps | **7,906 rps** | **+143%** | -60% |
| path-params (switch + Boa condition eval) | 2,098 rps | **8,486 rps** | **+305%** | -75% |
| thin-dsl (037 fast-path — engine bypassed) | 77,777 rps | 80,027 rps | +3% (parity) | -3% |

**2-4× throughput improvement + 60-80% latency reduction on Boa-hitting
DSLs.** Moves the JS ceiling from 1-3k rps into 6-9k rps range;
framework baseline (~95k rps on `/health`) unchanged.

### Deferred

- **Task 045 — pre-parsed script cache.** v1 attempted as per-session
  compiled-function cache. Wins on repetition-heavy DSLs (+11% on
  guarded) but regresses on unique-per-request DSLs (-15% on
  path-params) because Mutex + double-eval-on-miss net-loses when
  cache almost never hits. Reverted; moved to backlog with three
  documented redesigns (compile-at-DSL-load, cross-request pool via
  dedicated JS worker threads, threshold-based caching). Gated on an
  iterate-heavy corpus emerging OR the perf story needing more
  compound wins.

## [0.6.4] - 2026-07-19

### Added

- **Task 051 — pluggable ScriptEngine backends behind Cargo features.**
  Split `src/scripting/` into an engine-agnostic shell + two backend
  modules. Exactly one of `scripting-boa` (default) or `scripting-quickjs`
  compiled per build; both-or-neither triggers a clean `compile_error!()`
  instead of a spray of unresolved symbols.
- `scripting-boa` (default): unchanged behaviour. Boa 0.19, pure Rust,
  no CVE surface. The existing 142 tests + 99 DSL scenarios pass
  byte-identically to v0.6.3.
- `scripting-quickjs`: rquickjs 0.6 with `parallel + futures` features
  for Send + Sync context types. **Same 142 tests + 99 DSL scenarios
  pass on this backend too** — full corpus compatibility gate. NaN
  serialisation error message aligned to Boa's exact wording so
  scenarios that regex on error text stay portable.
- `book/src/framework/scripting-engines.md`: engine selection guide,
  known compatibility deltas (Number precision, Date parsing, regex
  flavor), why this split unblocks tasks 036 + 045.

### Consequence

Tasks 036 (per-request context pool) and 045 (pre-parsed Script cache)
— previously blocked on Boa's `!Send` types — become straightforward
small changes against the QuickJS backend. Reopening them is the next
sequenced work; combined expected impact is 5-10× on JS-heavy DSLs.

## [0.6.3] - 2026-07-19

### Findings

- **Task 047 spike answered YES.** `rquickjs` with `parallel + futures`
  features exposes `Send + Sync` Runtime and Context types. Verified
  by compile-time `assert_send<T>()` markers AND runtime tests that
  hold an AsyncContext across `.await` on a multi-thread tokio
  runtime and spawn it into another task. This unblocks tasks 036
  (per-request BoaContext pool) and 045 (pre-parsed Script cache)
  which were both blocked on Boa's `!Send` internals.
- Consequence: the compound-win path (potential 5-10× on Boa-hitting
  DSLs) is open via a QuickJS backend. Follow-up filed as task 051.

### Added

- Feature-gated dependency `rquickjs` behind `spike-quickjs` cargo
  feature (off by default). Enables `tests/spike_047_quickjs_send.rs`
  — 6 tests documenting the Send/Sync findings. Default build
  unchanged in size, dependencies, or behaviour.

### Filed

- **Task 051 — Adopt rquickjs as an alternative ScriptEngine backend**
  behind a mutually-exclusive `scripting-quickjs` feature flag. Once
  051 lands, tasks 036 and 045 become straightforward small changes
  rather than architectural refactors requiring dedicated OS worker
  thread pools.

### Deprecated (kind of)

- The Boa-perf roadmap's "dedicated JS worker thread pool" fallback
  path is now optional. If 051 delivers on the corpus-compatibility
  gate, we skip the worker-pool refactor entirely.

## [0.6.2] - 2026-07-19

### Added

- **Task 049 — HTTP/2 cleartext (h2c) over UDS**, opt-in via
  `uds_http_version: http2` (outbound) and `listeners: [..., {http2: true}]`
  (inbound). Both sides must speak the same version; there's no ALPN
  over cleartext. Client and server implementations both compile
  against hyper's http2 builders; 6 new integration tests verify
  round-trip, backwards-compat with h1, mismatched-version failures,
  and 32-way concurrent multiplexing.

### Perf

Measured A/B (3-run median, laptop, cross-instance sidecar hop):

| Version | rps | p50 |
|---|---:|---:|
| h1 pool | 5,121 | 12.4 ms |
| h2c | 4,910 | 13.0 ms |

**h2c is ~4% slower on this workload.** Honest finding: h2's
multiplexing win only materialises when a single caller fans out
many concurrent streams to the SAME target. The current bench
pattern makes one main→side call per inbound request; h1-with-pool
and h2-with-one-stream-per-request perform equivalently, with h2
losing on per-frame overhead.

Once task 040 (`parallel_http`) lands, h2's one-connection-N-streams
should beat h1's N-pooled-connections by 3-5× on the fan-out pattern.
Book chapter documents this and recommends the h1 default until 040
is available. `uds_http_version: http2` is opt-in for operators
already using fan-out via `iterate` around `http.<verb>`.

## [0.6.1] - 2026-07-19

### Added

- **Task 050 — UDS keep-alive connection pool.** Replaces v0.6.0's
  per-request handshake with a hyper-util `Client<UdsConnector,
  Full<Bytes>>` cached per unique socket path. Every socket gets its
  own connection pool; requests reuse warm connections instead of
  paying handshake cost every time. This is the fix v0.6.0's UDS
  path was missing — the A/B bench had shown pooled TCP loopback
  beating v1 UDS by 6%; v0.6.1 flips that to UDS winning by 6%.
  Defaults: 30s idle timeout, 32 idle connections per host. 3 new
  tests covering pool identity, sequential-reuse under load, and
  target-restart recovery.

### Perf

Measured A/B on the same sidecar-hop workload (3-run median,
developer laptop, cross-instance UDS vs TCP loopback):

| Transport | v0.6.0 | v0.6.1 | Δ |
|---|---:|---:|---|
| TCP loopback | 4,229 rps | 4,839 rps | +14% (laptop noise) |
| **UDS via alias** | 3,987 rps | **5,122 rps** | **+28.5%** |
| UDS-vs-TCP delta | -6% (worse) | **+5.8% (wins)** | |

p50 latency on UDS: 15.9 ms → 12.4 ms (-22%).

For headline-grade numbers, re-run on an isolated host per
`bench/AWS-RUNBOOK.md` — localhost variance is ±20%.

### Filed

- **Task 049 (h2c over UDS + TCP)** — the next transport-perf lever
  after 050. HTTP/1.1 head-of-line blocking caps per-connection
  throughput; h2 stream multiplexing eliminates it. Composes with
  050's pool infra.
- **Task 047 reframed** — QuickJS evaluation now framed as the
  potential unblocker for tasks 036 + 045 (which are blocked on
  Boa's `!Send` types). If `rquickjs::Context` is `Send`, adopting
  QuickJS unblocks the compound Boa-perf wins without needing a
  dedicated JS worker thread pool.

## [0.6.0] - 2026-07-19

### Added

- **Task 039 — perf benchmark suite.** `bench/` with 6 wrk-based
  scenarios (framework baseline, thin DSL, JS-heavy, path-params,
  cached-response, guarded), a runner (`bench/run.sh`) that boots
  the release binary on a configurable port and emits JSON, a
  median-of-N baseline capture (`bench/refresh-baseline.sh`), and
  a comparator (`bench/compare.py`) that gates on rps regression
  and warns on p50. 17 comparator tests. `.github/workflows/perf.yml`
  wired as workflow_dispatch (push-triggered gating deferred — GH-
  hosted runner variance is too high; documented in `bench/README.md`).
- **Task 042 — `single_flight` DSL step.** In-process coalescing of
  concurrent duplicate requests keyed on a DSL-computed string.
  First arrival becomes the leader, executes `do:`, broadcasts the
  outcome via a per-key `tokio::sync::broadcast` channel; concurrent
  followers subscribe and receive the same value. Same-instance
  only (cross-replica dedup is task 029's shared-store domain).
  8 integration tests; 1 dsl-test scenario; book chapter at
  [`book/src/dsl/steps/single_flight.md`](book/src/dsl/steps/single_flight.md).
- **Task 043 — Unix Domain Socket transport for inter-service hops.**
  DSLs stay portable: `http://alias/path` transparently routes via
  UDS when the operator maps `alias` in `unix_socket_map`. Explicit
  `unix://` URLs supported too. Inbound multi-listener mode via
  `listeners:` config; each listener runs its own accept loop, same
  Router serves all. Skips ~15-25 µs CPU + ~100-300 µs wall
  latency per hop vs TCP loopback. 8 outbound + 2 inbound + 6 URL-
  parser unit tests; book chapter at
  [`book/src/framework/inter-service-transport.md`](book/src/framework/inter-service-transport.md).
- **Task 044 — `http.<verb>` self-call short-circuit.** When an
  outbound URL resolves to Ruuter's own listener, dispatch in-process
  through the router instead of round-tripping via reqwest + TCP.
  Preserves guards, CSRF, path-param resolution, and response shape
  (byte-identical to network loopback). Every loopback synonym
  (`localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]`, `::1`) on the
  configured port matches automatically. 10 integration tests;
  book chapter at [`book/src/framework/self-call-optimization.md`](book/src/framework/self-call-optimization.md).

### Changed

- `DslRouter::build_axum_router(self)` still works; new
  `build_axum_router_from_arc()` on `Arc<Self>` for the self-call
  wiring path that needs the Arc alive after the axum router is
  built (task 044).
- Task backlog reshaped: `023` (http.patch, already implemented)
  and `025` (publishable artefact) moved to `tasks/done/`; `028`
  (JWT/TIM guard) and `030` (framework ETag validation) moved to
  a new `tasks/wont-fix/` folder for owner-declined-with-rationale;
  `040` (parallel_http) and `041` (first_n aggregation) moved to
  `tasks/backlog/` — dependent on a fan-out design that wasn't
  ready in this batch. Downstream-project naming stripped from
  `Filed` sections of `038`, `040`-`044` (Ruuter is a generic
  framework; per-project justification prose doesn't belong here).

### Not shipped (documented as follow-ups)

- Idempotency-Key cache consultation on self-calls (task 044)
- `force_network: true` DSL escape hatch (task 044)
- HTTP/2 over UDS (task 043)
- Streaming response-body size cap on UDS + self-call paths
- Cross-instance single_flight coalescing (task 042; needs the
  same shared-store design as task 029)
- Push-triggered CI perf gate (task 039; needs a dedicated
  bare-metal runner — GH-hosted variance is too high)

## [0.5.0] - 2026-07-18

### Added
- **Task 037 — literal fast-path in `ScriptEngine::evaluate()`.** Values
  that recursively contain no `${...}` and are not whole-string
  `$=...=` expressions now bypass Boa entirely, returning
  byte-identical output without constructing a `BoaContext` or running
  `setup_bindings`. Measured on `/samples/basic/hello` (a literal
  return string): **74,621 rps at p50 714µs** on this laptop, up
  from **4,796 rps at p50 13.30ms** on 0.4.0 — a 15.6× throughput
  increase / 18.6× latency reduction. Correctness argument:
  pre-037 evaluation of an expression-free value tree was already the
  identity function; the fast-path skips the identity work. Covered
  by 15 targeted unit tests plus `evaluate_tracked()`, which returns
  a per-call `boa_used: bool` so tests can observe fast-path firing
  without racing the process-global counter across parallel test
  binaries.
- CLI testkit tools: `dsl-lint` (validates every DSL in a tree,
  fails on error) and `dsl-test` (runs `.test.yml` scenarios against
  a live server). Wired into CI as the second and third gates after
  `cargo test`.
- mdBook reference at `book/` — LLM-oriented documentation covering
  every step, source, guard, and framework surface. Deployed to
  GitHub Pages on push to `dev`.

### Changed
- **License: MIT → Apache-2.0**, with a `NOTICE` file crediting
  Bürokratt's original Ruuter (Java) as the reference implementation
  this rewrite mirrors semantically.
- **Package renamed: `ruuter-rs` → `ruuter-on-rust`.** Cargo
  `name`, binary name, repository URL (`github.com/turnerrainer/Ruuter`),
  and Docker image tag all use the new name. There is no compat shim
  — pre-0.5.0 references to `ruuter-rs` need updating.
- README and DSL sample docs updated for the rename and the new
  license.

### Performance

Ad-hoc `wrk -t4 -c64 -d10s` on a 2-core laptop, same host, native
binaries built with `--release`:

| Route | 0.4.0 | 0.5.0 | Change |
|---|---:|---:|---|
| `/health` (framework baseline) | 98,233 rps | 102,723 rps | +4.6% (noise) |
| `/samples/basic/hello` (literal DSL) | 4,796 rps | **74,621 rps** | **+1456%** |

Routes whose DSLs still contain runtime `${...}` expressions are
unchanged in this release — the fast-path only fires when the value
tree is expression-free. Task 037's benefit scales with the fraction
of expression-free values in a project's DSL corpus.

### Deferred / Blocked

Three related tasks were investigated on this branch and documented
rather than implemented, because Boa 0.19's `Context` and `Script`
types embed `Rc`s (`!Send + !Sync`), which cannot cross the
framework's async `.await` boundaries:

- **Task 036 — per-project `BoaContext` pool.** Requires dedicated
  OS worker threads for JS execution, not a field on
  `ExecutionContext`. See `tasks/todo/036-boa-context-pool-per-project.md`.
- **Task 045 — pre-parse expressions at DSL load.** `Script::parse`
  requires a live `&mut Context` and the resulting `Script` holds a
  `Realm` in a `boa_gc::Gc`. Same unblock path as 036.
- **Task 046 — load-time static evaluation.** Not blocked, but a
  corpus survey showed zero `${...}` expressions in `DSL/samples/**`
  would be hoisted by either the ultra-safe subset or the full
  allow-list version. Deferred until a corpus that would benefit
  emerges.

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

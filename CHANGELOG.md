# Changelog

All notable changes to Ruuter-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0-rc] - 2026-09-12

Sixteen h2ck.me v1 backlog items shipped in one batch (T-1..T-16,
PRs #101–#116). Two breaking Rust API changes (T-4
`StepEngine::new` signature, T-5 `StateStore` return types), two
client-facing behaviour changes (T-10 multipart map-key, T-15
wrong-method 405 + `Allow:`), new operator surfaces
(`ruuter-doctor` binary, `RUUTER_OFFLINE=true` env, inbound
`TimeoutLayer`), P0 security hardening (T-1 / T-2 / T-3), and
boot-time WARNs (T-9 OWASP, T-13 CSRF).

Semver-minor bump. Every item below has its own PR with a
dedicated regression-test file; ~130 new test functions total.
See CLAUDE.md's "Handling a breaking change" section for the
process this batch followed.

### Fixed

- **h2ck.me v1 T-1 — `http_response_size_limit` default resolves
  to `None` on operator YAML.** Pre-fix,
  `#[serde(default)]` on `pub http_response_size_limit:
  Option<usize>` fell back to `Default::default()` → `None`, so any
  operator whose `ruuter.yaml` omitted the field silently ran with
  the outbound response-body cap disabled. `HttpClient::request`
  then read via `response.bytes().await`, which allocates the whole
  upstream body — a misbehaving Resql/TIM sidecar (or attacker-
  controlled upstream, when the deployment allowed one) could OOM
  the process by returning a very large body. `AppConfig::default()`
  did carry `Some(16 * 1024 * 1024)`, but only the "no ruuter.yaml
  found" boot path used it; the operator-YAML path did not.

  Post-fix, `#[serde(default = "default_http_response_size_limit")]`
  binds an absent field to `Some(16 * 1024 * 1024)`. Explicit
  `http_response_size_limit: null` still deserialises to `None` so
  the uncapped opt-in survives for internal-only deployments; a
  new `warn_on_raw_config_notes` boot WARN names the field when
  that opt-in is exercised, so a stray null (typo, copy-paste of a
  Java template that used null as a sentinel) surfaces at boot in
  the same log stream as "Loaded config from …". Detection uses a
  raw-YAML scan (`raw_config_notes`) because the parsed
  `AppConfig` cannot distinguish "field absent" from "field
  explicitly null" — both deserialise identically once the default
  fn runs. `AppConfig::load_or_default_with_notes` returns the
  observation struct alongside the parsed config; the pre-existing
  `AppConfig::load_or_default` delegates so no external caller
  broke.

  Regression coverage: 20 test functions in
  `tests/issue_T1_http_response_size_limit_default.rs` pin the
  full matrix — absent, empty document, numeric, large numeric,
  zero, explicit null, YAML `~` shorthand, malformed YAML, and
  subscriber-driven tests that capture the actual `tracing::warn!`
  output on each of those inputs.

- **h2ck.me v1 T-2 — UDS outbound reads response body unbounded
  regardless of `http_response_size_limit`.** Pre-fix, both UDS
  transports (`http_client/uds.rs::request_over_unix` and
  `http_client/uds_pool.rs::request_over_unix_pooled`) called
  `.into_body().collect().await.to_bytes()` with no cap; a POST-HOC
  size check in `HttpClient::enforce_status_and_size` then rejected
  based on the already-buffered `HttpResponse`. Result: a
  misbehaving trusted sidecar (Resql/TIM bug — not compromise) could
  OOM Ruuter by returning a very large body, and T-1's config-default
  fix did not close the seam because the check ran after the read.

  Post-fix, both UDS paths now:
    1. Preflight `Content-Length` against `response_size_limit` and
       reject with `RuuterError::HttpRequest("uds upstream declared
       body N bytes exceeds http_response_size_limit M")` before
       reading the body. Skips the buffering entirely for
       oversized-declared responses. Mirrors the TCP path at
       `http_client/mod.rs`.
    2. Wrap `res.into_body()` in `http_body_util::Limited::new(body,
       cap)` before `.collect()`. Chunked / unknown-length responses
       that would previously buffer past the cap now abort
       mid-stream at the cap boundary and return
       `RuuterError::HttpRequest("uds upstream response body
       exceeded http_response_size_limit M")`.
    3. The post-hoc `enforce_status_and_size` in `HttpClient` is
       deleted — dead code once the cap is enforced upstream.

  Public-API surface: `request_over_unix` and
  `request_over_unix_pooled` gain a trailing `response_size_limit:
  Option<usize>` argument (matches how the TCP path threads the cap
  through `HttpClient::response_size_limit`). New test-only builder
  `HttpClient::with_response_size_limit(Option<usize>)`. Neither is
  a breaking change for internal callers; external callers of the
  pub UDS transports (none known) need to pass a cap or `None`.

  Regression coverage: 12 test functions in
  `tests/issue_T2_uds_body_cap.rs` — Content-Length preflight,
  mid-stream Limited abort on chunked bodies (no Content-Length),
  `None` cap opt-out reads full body, cap-equal-to-body edge (off-
  by-one), cap = 0 rejects any non-empty body, JSON-decode path
  survives Limited wrap (#98 interaction), pooled + non-pooled
  parity, alias-map + `unix://` routing parity.

- **h2ck.me v1 T-3 — DNS-rebinding TOCTOU in
  `HttpClient::check_ssrf`.** Pre-fix, `check_ssrf` resolved the URL
  host via `tokio::net::lookup_host` and rejected the request if any
  candidate address was private / link-local, then handed the URL
  back to reqwest. Reqwest performed a FRESH resolve at connect
  time; an attacker controlling the DNS record could flip the answer
  between check and connect (`public → check pass → connect fires
  → private`), completing a metadata-SSRF that the F2 fix was
  designed to block.

  Post-fix, `check_ssrf` returns a new internal `SsrfResolution`
  enum: `NoPinning` for IP-literal URLs / allowlist-approved hosts /
  `block_private_networks=false`, and `Pinned { host, addrs }` for
  the hostname-with-block-active path. The caller
  (`build_pinned_client`) constructs a per-request `reqwest::Client`
  via `ClientBuilder::resolve(host, addr)` for every addr that
  passed the check, so the connect is bound to those exact
  addresses. A fresh DNS answer at connect time cannot flip the
  target. Multi-A-record failover still works because all resolved
  addrs get pinned. The shared `HttpClient::client` pool is
  preserved for the no-pinning path — pinning applies only when
  DNS actually ran.

  No breaking changes for DSL authors. Config surface unchanged —
  the pinning is driven off the pre-existing
  `internal_requests.block_private_networks` flag.

  Regression coverage: 8 test functions in
  `tests/issue_T3_dns_rebinding_pinning.rs` — the `.resolve()`
  primitive really pins the connect (proves the reqwest mechanism),
  multi-addr disambiguation by port, IP-literal URL skips pinning,
  `block_private_networks=false` skips pinning, allowlist-approved
  host skips pinning, hostname resolving to a private IP still gets
  rejected (F2 regression pin), rejection message names the
  resolved IP (proves the resolver ran), per-request check runs
  independently (no first-request-cache-poisoning).

### Changed (breaking)

- **h2ck.me v1 T-4 — `StepEngine::new` now requires a
  `SharedGuards` handle as a positional argument.** Pre-fix,
  `guards: Option<SharedGuards>` on `StepEngine` was populated
  post-hoc via a `with_guards(SharedGuards, GuardMode)` builder;
  any caller that forgot to call `.with_guards` silently disabled
  `template:`-step guard enforcement — reopening the h2ck.me H1
  bypass ("public DSL templates into a guarded admin route").
  Nothing at compile time prevented the omission; the mistake had
  to be caught by test coverage that specifically exercised the
  guarded template path.

  Post-fix: `pub fn new(http_client: HttpClient, guards:
  SharedGuards, guards_mode: GuardMode) -> Self`. The `with_guards`
  builder is deleted. Callers with legitimately no guards pass a
  new module-level helper `empty_shared_guards()` — an explicit
  call reviewers can spot. Any future call site that forgets to
  wire guards FAILS TO COMPILE, which is the regression pin.

  Migration for external callers (internal tests + main + testkit
  + dsl-test all updated in this PR):
  ```diff
  - let engine = StepEngine::new(http_client)
  -     .with_guards(shared_guards, cfg.guards.mode);
  + let engine = StepEngine::new(http_client, shared_guards, cfg.guards.mode);
  ```
  For test fixtures that never had guards:
  ```diff
  - let engine = StepEngine::new(http_client);
  + let engine = StepEngine::new(
  +     http_client,
  +     ruuter_on_rust::steps::engine::empty_shared_guards(),
  +     cfg.guards.mode,
  + );
  ```

  New public helper: `ruuter_on_rust::steps::engine::empty_shared_guards()
  -> SharedGuards`. Also re-exported through the crate.

  Regression coverage: 4 test functions in
  `tests/issue_T4_stepengine_guards_required.rs` document the
  compile-time contract (the pin IS the compile error a future
  refactor would hit) and verify `empty_shared_guards()` returns
  a valid handle, `applicable_guards_for` still runs against it,
  and populated handles reach the engine correctly. ~50 pre-
  existing test fixtures updated in-place to the new signature.

### Added

- **h2ck.me v1 T-5 — process-wide state store now supports a
  per-project entry cap.** Pre-fix, `StateStore` was an unbounded
  `DashMap<StateKey, Value>` — a DSL that keyed state on request
  data (`state.set(key = ${incoming.body.foo})`) could OOM the
  process by growing the map without bound. Post-fix, new config
  key `state.max_entries_per_project` (default `100_000`, `null`
  = unbounded) bounds each project independently.

  - `StateStore::set` now returns `Result<()>`; a new-key insert
    past the cap fails with `RuuterError::InvalidStep("state.set
    rejected for project 'X': entry count N reached the cap
    max_entries_per_project=M (h2ck.me v1 T-5). ...")`.
    Existing-key updates are always allowed (no count change).
  - `StateStore::update` follows the same contract — signature
    changed from `Value` to `Result<Value>`; new-key inserts
    honour the cap, existing-key updates never trip it.
  - `StateStore::delete` decrements the per-project count.
  - At **80% of the cap**, the store emits ONE `tracing::warn!`
    line naming the project + entries + cap. Subsequent inserts up
    to the wall don't spam. Once the wall hits, each rejected
    insert surfaces to the DSL author via the step-level error.
  - New helpers: `StateStore::with_config(&StateConfig)`,
    `with_max_entries_per_project(usize)`,
    `project_entry_count(&str)`, `max_entries_per_project()`,
    `project_stats()`.
  - New public struct `ProjectStats { project, entries, cap }`
    powers a new admin endpoint `GET /_/state-stats` (mounted
    under `admin_router`, `RUUTER_ADMIN_ENABLED=true` required).
    Response shape: `{ cap, totals: { projects, entries },
    projects: [ { project, entries, cap, used_pct } ] }`.
  - `main.rs`, `src/testkit/harness.rs`, and `src/bin/dsl_test.rs`
    wire the store via `with_config` (or `.expect()` on set seeds).

  Migration: DSL authors who keyed state on unbounded request data
  and relied on the pre-T-5 grow-forever behaviour must either
  move to a bounded key namespace, add explicit `state.delete` to
  clean up, or raise the cap. The default of 100_000 is
  comfortable for legitimate DSLs (session tables with sensible
  TTL, dedup markers, counters); DSLs that hit it are almost
  certainly the very footgun T-5 closes.

  Regression coverage: 20 test functions in
  `tests/issue_T5_statestore_bounded.rs`.

### Changed

- **h2ck.me v1 T-6 — `RUUTER_HTTP_REWRITE` is now gated behind a
  new `dev-http-rewrite` Cargo feature in release builds.** Pre-
  fix, the env-var-driven URL rewriter shipped in every release
  binary. An operator who accidentally set the env var in prod
  silently disabled `check_ssrf` for the rewritten origin — the
  h2ck.me M2 boot WARN was a mitigation, not a fix.

  Post-fix, `rewrite_url_for_tests` and
  `rewrite_env_is_active_in_release` are conditionally compiled
  behind `#[cfg(any(debug_assertions, feature = "dev-http-rewrite"))]`.
  The non-feature branch replaces both with no-op stubs (const
  `RUUTER_HTTP_REWRITE_ENV` still exported — it's just a string).
  A stock `cargo build --release` produces a binary in which the
  rewriter code is not present; setting the env var in prod has
  literally no effect on outbound URL routing.

  Debug builds (`cfg!(debug_assertions)`) and release builds with
  `--features dev-http-rewrite` retain the pre-fix behaviour so
  `dsl-test`, mock-http harnesses, and staging binaries that
  legitimately need URL redirection keep working. The M2 WARN
  logic in `main.rs` still calls
  `rewrite_env_is_active_in_release()`, but in a stock release
  binary that always returns `false`, so the WARN is a no-op —
  matching the reality that the rewriter isn't there to fire.

  Regression coverage: 4 test functions in
  `tests/issue_T6_rewrite_feature_gated.rs` — env-var name const
  is stable in both builds, `rewrite_env_is_active_in_release` in
  a debug-assertions-on binary always returns `false` (empty +
  set env), and the debug-mode rewriter still redirects outbound
  URLs to a local server. Release-build no-op verified by
  `cargo build --release` and `cargo build --release --features
  dev-http-rewrite` in CI.

### Added

- **h2ck.me v1 T-7 — inbound request wall-clock timeout via
  `tower_http::timeout::TimeoutLayer`.** Pre-fix, every inbound
  request rode a tokio task with no wall-clock ceiling. The
  engine's `max_step_recursions` / `max_iterations` / per-outbound
  timeouts covered the DSL-execution phase; they didn't cover a
  slow-body / slow-header probe (Slowloris-style attack, or a
  client that never finished sending) that tied up a worker
  before any DSL ran.

  Post-fix: new config field
  `incoming_requests.request_timeout_ms`, default `Some(30_000)`
  (30 seconds). Applied via `TimeoutLayer` around the DSL
  fallback, layered after CORS so pre-flight OPTIONS still
  respond fast. Breaches surface as `408 Request Timeout`
  (tower_http's default in axum 0.7). Explicit `null` in
  ruuter.yaml opts back into the pre-T-7 no-timeout behaviour.

  Public-API surface: `IncomingRequestsConfig` gains a required
  field `request_timeout_ms: Option<u64>`. Test fixtures that
  built the struct directly (`security_hardening.rs`,
  `security.rs`) updated to pass `None`.

  Regression coverage: 8 test functions in
  `tests/issue_T7_inbound_request_timeout.rs` — config default,
  absent-field, explicit-numeric, explicit-null, slow-handler
  gets 408/504 within timeout, fast-handler still 200, null
  timeout lets slow handler complete, generous timeout lets
  short handler complete without waiting for the cap.

### Added

- **h2ck.me v1 T-8 — early-reject on `Content-Length > cap` before
  any body bytes are read.** `axum::body::to_bytes` already rejects
  mid-stream at 16 MiB via `http_body_util::Limited` (the RUNTIME-
  FINDINGS "100 → 117 MB RSS on a 100 MB POST" was hyper socket-
  buffer overhead, not eager buffering). Real (smaller) improvement:
  when the client explicitly declares a Content-Length above the
  cap, we can 413 the request before ANY body reads, cutting out
  hyper's socket-buffer accumulation entirely.

  Post-fix, `handle_request` inspects the `Content-Length` header
  and returns `413 Payload Too Large` with a structured JSON body:
  ```json
  { "error": "body_too_large", "declared": N, "cap": 16777216,
    "message": "declared Content-Length N exceeds inbound body cap
                16777216 (h2ck.me v1 T-8)" }
  ```
  The `>` comparison is strict — a declared CL exactly at the cap
  is allowed. Missing / malformed CL falls through to the existing
  mid-stream Limited behaviour. Preserves the `#92`-style
  structured-error shape callers expect.

  Regression coverage: 6 test functions in
  `tests/issue_T8_content_length_preflight.rs` — oversized CL →
  413 with structured JSON, CL == cap passes preflight, small CL
  reaches DSL, missing CL reaches DSL, malformed CL doesn't
  trigger preflight, DSL never runs on preflight reject.

### Added

- **h2ck.me v1 T-9 — boot WARN when a non-loopback listener is
  configured but `response_default_headers` doesn't include the
  OWASP baseline (`X-Content-Type-Options`, `X-Frame-Options`,
  `Strict-Transport-Security`, `Referrer-Policy`).** The header
  machinery existed and `book/src/ops/security-checklist.md`
  documented the posture, but there was no boot-time signal — an
  operator who exposed Ruuter on `0.0.0.0:8080` and forgot to add
  the baseline never saw a warning.

  Post-fix, `warn_on_stale_config_fields` now fires a WARN naming
  each missing header and pointing at
  `book/src/ops/security-checklist.md`. Scoped to
  network-reachable listeners — loopback (`127.0.0.1`, `::1`,
  `localhost`), UDS, and mixes thereof never trigger it.
  Case-insensitive matching on header names.

  Public helpers exposed for tooling / testing:
  `has_non_loopback_listener(&AppConfig) -> bool`,
  `missing_owasp_baseline_headers(&AppConfig) -> Vec<&'static str>`,
  and the const `OWASP_BASELINE_HEADERS`.

  Regression coverage: 18 test functions in
  `tests/issue_T9_owasp_baseline_headers.rs`.

### Changed (behavior)

- **h2ck.me v1 T-10 — multipart `Content-Disposition` filename no
  longer wins over the field `name` as the `incoming.body` map
  key.** Pre-fix, `filename.or(name)` in `parse_multipart_body`
  meant an attacker-controlled filename (path-traversal shape,
  Unicode homoglyph, empty string) became the key that DSLs read
  via `${incoming.body.<key>}`. In-framework the key is just a
  JSON-map key, but downstream DSLs that forward the key to a
  trusted system (path building, log line, cache key) inherit
  the nastiness.

  Post-fix: `name.or(filename)`. The stable field name wins; the
  filename is used as a fallback ONLY when the field has no
  `name=`. Anonymous fields still fall back to `"part"`.

  **DSL-author migration:** any DSL that previously read
  `${incoming.body['<filename>']}` from a multipart upload must
  switch to `${incoming.body.<field-name>}`. Standard form
  contracts already do this (`name="file"` etc.); ad-hoc DSLs
  that keyed on `filename=` need one-line updates.

  Regression coverage: 8 test functions in
  `tests/issue_T10_multipart_field_name_key.rs` — normal case,
  path-traversal filename (`../etc/passwd`) with field name,
  Unicode-homoglyph filename with field name, empty filename with
  field name, no filename, no field name falls back to filename,
  fully anonymous falls back to `"part"`, multiple same-name
  fields last-wins. The pre-existing `audit_content_types.rs`
  test (`inbound_multipart_form_data_parses_file_parts`) updated
  in-place to the new field-name-as-key contract.

### Added

- **h2ck.me v1 T-11 — new `ruuter-doctor` binary for pre-boot
  config sanity checking.** Loads ruuter.yaml + env vars, runs
  the boot-path WARN registry, reports a CI-actionable exit code:
  0 clean, 1 warning(s) would fire, 2 unparseable, 3 bad args.
  Captured WARNs echo to stdout for grep-friendly CI. Ships in
  the container alongside `dsl-lint` / `dsl-test`.

  New public helper `load_or_default_via_env_or_path(Option<&Path>)`
  in `ruuter_on_rust::config`.

  Regression coverage: 6 test functions in
  `tests/issue_T11_ruuter_doctor.rs` (subprocess-driven), plus
  test fixtures at `tests/fixtures/{clean-defaults,
  insecure-defaults, unparseable}.yaml`.

### Changed

- **h2ck.me v1 T-12 — every HTTP DSL sample under `DSL/samples/`
  now carries a `declaration:` block.** Pre-fix, `grep -rln
  '^declaration:' DSL/samples/` returned 2 of 58 samples;
  Ruuter's own samples didn't demonstrate the feature Ruuter
  advertises. Post-fix, 54 additional samples got a minimal
  declaration (description + `additive: true` allowlist for HTTP
  DSLs, `override_ancestors: false` for guards) so DSL authors
  reading the tree have a working reference.

  The `additive: true` posture means the declaration is
  documentation-only — undeclared fields still pass through and
  the sample keeps its existing runtime behaviour. Authors who
  want strict rejection flip `additive` to `strict: true`.

  WS, trigger, and cron samples are intentionally unchanged —
  they're not OpenAPI-routable and `warn_on_missing_declarations`
  already skips them.

  Regression coverage: 2 test functions in
  `tests/issue_T12_samples_have_declarations.rs` — load
  `DSL/samples/` and assert `warn_on_missing_declarations` returns
  0, plus a per-DSL walk that asserts `dsl.declaration.is_some()`
  for every HTTP-method-bucketed DSL.

### Added

- **h2ck.me v1 T-13 — boot WARN when `csrf.allowed_origins` is
  empty.** Pre-fix, empty `allowed_origins` silently disabled the
  Origin/Referer CSRF check for state-changing methods; documented
  at `book/src/framework/csrf.md` but no boot-time signal.
  Post-fix, `warn_on_stale_config_fields` names the field and
  points at the doc, matching the fleet's default-off-warn pattern.
  5 tests in `tests/issue_T13_csrf_empty_origins_warn.rs`.

### Added

- **h2ck.me v1 T-14 — `RUUTER_OFFLINE=true` env for hard-stubbed
  outbound HTTP.** `dsl-test` already has a MockServer + per-test
  `http_rewrite:` for hermetic testing; production and staging
  boot had no analogue. Post-fix, setting the env to any truthy
  value (`true`, `1`, `yes`, `on`, case-insensitive) short-
  circuits every outbound `http.*` step BEFORE `check_ssrf` /
  connect / UDS. Response shape matches issue #89's
  transport-error stub so DSL `check_*` switches keyed on
  `${result.response.status == 0}` fire in offline mode too:

  ```json
  { "status": 0,
    "error": "offline",
    "body": { "error": "offline", "message": "..." },
    "headers": {} }
  ```

  `main.rs` emits a boot WARN whenever the env is set so ops
  teams don't confuse offline-mode zero-status responses for
  a real upstream outage.

  New public helpers in `ruuter_on_rust::http_client`:
  `ruuter_offline_env_active() -> bool`,
  `offline_stub_response() -> HttpResponse`, and the const
  `RUUTER_OFFLINE_ENV`.

  Regression coverage: 16 test functions in
  `tests/issue_T14_ruuter_offline_env.rs` pin the truthy-value
  classifier (unset, empty, false, 0, true, TRUE, 1, yes, on,
  junk), stub shape (status/error/body/headers), and end-to-end
  behaviour through `HttpClient::request`. Env mutations are
  serialised through a process-wide mutex to avoid races with
  parallel tests.

### Changed (behavior)

- **h2ck.me v1 T-15 — wrong method on a KNOWN path now returns
  `405 Method Not Allowed` with an `Allow:` header (RFC 7231
  §7.4.1).** Pre-fix, `PUT /svc/things` when only `GET /svc/things`
  was routed returned `404`. Post-fix: `405` with
  `Allow: GET, POST, PUT` (sorted alphabetically) + body
  `{"error":"Method Not Allowed","allow":["GET","POST","PUT"]}`.

  Paths that don't resolve for ANY method continue to return
  `404`. Unknown projects still `404`. Path-param resolvers
  work: `PATCH /svc/things/42` when only `GET /svc/things.yml`
  matches via suffix-stripping → `405 + Allow: GET`.

  Also fixed a pre-existing bug: the Err-branch of the response
  builder dropped `extra_headers` on the floor. Now applied on
  every branch.

  Public: new `DslRouter::methods_allowed_for_path(project, path)
  -> Vec<String>` for tooling.

  Regression coverage: 6 test functions in
  `tests/issue_T15_405_allow_header.rs`. Two pre-existing
  `DSL-tests/framework/{fallback-404, method-allowlist}.test.yml`
  scenarios updated to the new 405 contract.

- **h2ck.me v1 T-16 — unknown-project 404s no longer echo the
  client's URL path segment as `dsl.project` in the trace span /
  access log.** Pre-fix, `handle_request` set `dsl.project` from
  the raw first URL segment. An attacker probing `POST
  /candidate-name/foo` for every candidate name saw their guess
  reflected in structured logs — a mild project-name enumeration
  signal for operators consuming the logs.

  Post-fix, `dsl.project` is populated from
  `router.dsls.contains_key(first_segment)`: known projects
  appear verbatim; unknown first-segments (including the empty
  string) render as `<unknown>`. The full URL path is still in
  `http.route` so debugging isn't impaired — the fix is scoped
  to the semantic `dsl.project` field only.

  Regression coverage: 4 test functions in
  `tests/issue_T16_project_trace_leak.rs` — known project
  verbatim, unknown project → `<unknown>` (and NOT the client's
  segment), `http.route` preserved for debugging, empty path
  falls through to `<unknown>`.

## [0.9.16-rc] - 2026-09-11

### Changed

- **Issue #98 — `http.*` response-body decode is now driven by
  the upstream `Content-Type` header instead of a
  parse-JSON-and-see-what-sticks heuristic.** Pre-fix, every
  response body was fed through `serde_json::from_slice`
  regardless of `Content-Type`; parse-success bound a structured
  value, parse-failure bound a `Value::String`. Two subtle
  surprises fell out:
    - A `text/plain` (or missing-`Content-Type`) response whose
      body happened to be valid JSON — `123`, `null`, `true`,
      `"hello"`, `{"a":1}` — reached the DSL as a JSON number /
      null / bool / string / object, not as the raw text the wire
      declared.
    - The UDS transport (both single and pooled) silently
      discarded non-JSON payloads: `serde_json::from_slice(...).ok()`
      returned `None`, which surfaced in the DSL as JSON null,
      dropping the response body entirely. The TCP path already
      handled this via a `Value::String` fallback (issue #23); the
      UDS path did not.

  Post-fix, all three transports (TCP, UDS, pooled UDS) share
  `decode_response_body`, which inspects `Content-Type` before
  choosing:

  | Response `Content-Type`             | `response.body` type   |
  |-------------------------------------|------------------------|
  | `application/json` (± `; …`)        | parsed JSON            |
  | `application/*+json`                | parsed JSON            |
  | anything else / missing             | UTF-8 lossy string     |
  | (any Content-Type) empty bytes      | `""` (preserves #63)   |

  When the upstream declares `Content-Type: application/json` but
  the body fails to parse (a gateway-502 pattern where the proxy
  returns HTML with a lying Content-Type), Ruuter emits a WARN
  naming the parse error and falls back to a raw string so the DSL
  can still forward / inspect the payload. New public helpers:
  `ruuter_on_rust::http_client::content_type_is_json` and
  `decode_response_body`.

  **Behaviour change for DSL authors:** if a route relied on the
  old byte-heuristic to parse JSON out of a `text/plain` or
  missing-`Content-Type` upstream, the value now arrives as a
  string. Two migration paths:
    - Preferred: fix the upstream to send `Content-Type:
      application/json`.
    - Otherwise: `${JSON.parse(r.response.body)}` in the DSL.

  Tests: 23 test functions in
  `tests/issue_98_content_type_decode.rs` covering the acceptance
  matrix (2xx JSON, `application/problem+json`, JSON arrays,
  `; charset=utf-8`, `text/plain` with a JSON-shaped body, invalid
  JSON under `application/json`, `text/xml`, empty body across
  Content-Types, case-insensitive header lookup, the
  `content_type_is_json` matcher, three UDS regression pins that
  exercise the real transport with an in-process axum server on a
  temp socket — one for the non-JSON-becomes-null bug that pre-#98
  UDS silently exhibited — and a `content_type: json_override`
  test that pins the "force JSON decode regardless of upstream
  Content-Type" opt-in path).

## [0.9.15-rc] - 2026-09-10

### Added

- **Issue #90 — supported JavaScript subset is now documented and
  test-verified.** `book/src/dsl/expressions.md` rewritten with the
  full "supported constructs" table: primitives, strings, arrays,
  objects, conversion, JSON, math, regex, date, functions. Every
  entry is pinned by `tests/issue_90_js_subset.rs` (26 test
  functions, ~100 assertions) that runs against BOTH the Boa and
  QuickJS backends on every release-gate cycle — a regression on
  either engine fails CI. New "Deliberately unsupported" section
  names the categories that will not be added (`console.*`,
  `fetch`, `require`, `eval`, `new Function`, async / Promise,
  `setTimeout`, filesystem / process). "Adding a construct to the
  supported list" section documents the empirical-verification
  workflow: add a row to the test file, get it green on both
  backends, PR to update the doc.

- **Issue #91 — YAML gotchas doc + `dsl-lint` scalar-quoting
  warnings across five hazard classes.** New
  `book/src/dsl/yaml-gotchas.md` page covers the YAML plain-scalar
  edge cases that trap DSL authors (`: ` inside a ternary, `, ` in
  flow context, `#` comment starts, block-scalar indicators, multi-
  line continuations, Unicode homoglyphs). The common thread:
  silent misparse, no error near the offending line, wrong value on
  the wire. `dsl-lint` now programmatically emits a WARNING (never
  an error) for all five classes:
    1. `: ` inside an unquoted `${…}` (mapping-value indicator —
       reporter's motivating ternary case)
    2. ` #` inside an unquoted `${…}` (comment cut)
    3. `,` inside a `${…}` sitting in flow context
       (`stamp: { x: ${format(a, b)} }`) — block-context `,` is
       left alone to avoid false-positive on legitimate arrow-fn
       comma-lists
    4. Values starting with reserved YAML metasyntax (`!`, `&`,
       `*`, `%`, `@`, backtick) — `[` and `{` legitimately start
       flow containers and are not flagged
    5. Unicode homoglyphs in structural YAML: fullwidth colon
       (U+FF1A), en dash (U+2013), em dash (U+2014), hyphen
       (U+2010) — scoped to before the first ` #` comment-start
       outside quotes and skipping quoted regions, so em dashes in
       doc comments / quoted prose don't false-positive
  The `${…}` inner checks enumerate every occurrence on the line,
  not just the top-level `<key>: <value>` split, so expressions
  embedded in flow-mapping shapes are inspected too. Check runs on
  every file INCLUDING ones that fail YAML parse, so the "wrap in
  quotes" remediation surfaces alongside the generic "mapping
  values not allowed" message serde-yaml emits. Tests: 19 cases in
  `tests/issue_91_yaml_scalar_quoting.rs` covering every class
  plus false-positive avoidance for comments, trailing comments,
  and quoted strings.

### Fixed

- **Issue #89 — `http.*` transport failures are now catchable by the
  DSL.** Pre-fix, connection-refused / DNS / TLS handshake / read-
  or-write timeout on an `http.get` / `http.post` (etc.) step
  propagated `reqwest::Error` as `RuuterError::Http`, aborted the
  whole run, and produced Ruuter's generic 500 response — the DSL
  author's `check_*` switch never ran, so semantic 502 responses
  from gateway / adapter DSLs were impossible for the availability-
  failure class. Post-fix, the transport error is surfaced in-band
  as a stub `HttpResponse { status: 0, body: {error, message},
  headers: {}, error: Some(kind) }`, which the http step binds to
  the DSL's `result:` — the author can then either branch on
  `${result.response.status == 0}` in a subsequent `check_*` switch
  (the reporter's motivating pattern), inspect the specific kind via
  `${result.response.error == 'timeout'}`, or wire an `error:`
  handler on the step. Fall-through to `next:` when neither `error:`
  nor an inspection is set. Policy-level pre-flight rejections (SSRF
  blocked, host-allowlist denial, malformed URL, response-size cap)
  still raise — they are ops decisions, not availability events, and
  making them catchable would leak internal reachability. The
  allow-list-miss path (real upstream response with a status outside
  `http_codes_allow_list`) is unchanged: still raises when no
  `error:` handler is set. New public helper:
  `http_client::classify_transport_error` mapping `reqwest::Error`
  to stable short kinds (`timeout`, `connect`, `request`, `body`,
  `decode`, `unknown`). New field on `HttpResponse`:
  `error: Option<String>`. Tests: 10 cases in
  `tests/issue_89_http_transport_catch.rs` — connect-refused,
  timeout, `error:` routing, fall-through, allow-list back-compat
  (both with and without `error:`), successful-response shape, stub
  body shape, and a documentation-pin on the stable kind list.
- **Issue #92 — `stop_in_case_of_exception` no longer WARNs on every
  boot for operators who never touched the field.** The Rust `bool`
  Default is `false`, so `#[serde(default)]` on the field
  deserialised an absent value as `false` and tripped
  `warn_on_stale_config_fields` (whose guard is `!config.stop_in_case_of_exception`).
  Fix uses `#[serde(default = "default_stop_in_case_of_exception")]`
  returning `true`, matching the engine's actual behaviour (always
  halts on step error). Explicit `false` in `ruuter.yaml` still
  WARNs — that's the intended surface for "you set a value we
  can't honour."

## [0.9.14-rc] - 2026-09-09

Three fixes on top of v0.9.13-rc: `dsl-lint` / `dsl-test` shipped in
the image (#83), template-step null headers (#85), and step-key-list
drift prevention (#82). Release-gate green: 541 passed / 0 failed /
3 ignored across 68 test binaries, dsl-lint 64 files clean, dsl-test
100/100, cargo audit 0 warnings, mdbook builds.

**Behaviour change surface (grep before upgrading):**

- The published image now includes `dsl-lint` and `dsl-test` under
  `/usr/local/bin/`. Any container harness that inspects the image's
  file list will see two extra binaries (~15 MB); nothing existing
  is renamed or removed.
- A `template:` step whose `headers:` map contains a value that
  evaluates to `undefined` no longer forwards the header at all.
  Previously that entry became `header: "null"` (four-byte string)
  on the child DSL's `incoming.headers`; downstream http steps
  forwarding the child's headers could then send `header: null` on
  the wire. Any DSL that intentionally relied on the string `"null"`
  as a header value must send it explicitly (`headers: { x: 'null' }`).

### Changed

- **Issue #82 — step-key list is now a single source of truth.** The
  parser's `ACTION_STEP_KEYS` and the linter's `STEP_KEYS` moved to
  `src/steps/mod.rs`, so future step primitives can't land in one
  place and be silently unrecognised in the other (the two-year
  `ws_tag:` drift PR #80 fixed). A unit test pins the invariant
  `ACTION_STEP_KEYS = STEP_KEYS \ {"declaration"}`. An integration
  test (`tests/issue_82_dsl_lint_step_recognition.rs`) invokes the
  shipped `dsl-lint` binary against a fixture DSL exercising every
  step primitive and asserts exit 0 — any drift trips this test.
  New shipped sample `DSL/samples/WS/inbound/roles.yml` demonstrates
  `ws_tag:` + `ws_send.broadcast_where`, giving the release-gate
  `dsl-lint DSL/samples` check something to trip on if `ws_tag:`
  ever regresses out of the linter's accept-list again.

### Added

- **Issue #83 — `dsl-lint` and `dsl-test` now ship in the published
  image.** The same `cargo build --release` that produces
  `ruuter-on-rust` already produces both CI tools; they now COPY into
  the runtime layer with symlinks under `/usr/local/bin/` so a
  downstream CI can `docker run --rm -v "$PWD:/w" -w /w
  turnerrainer/ruuter:<tag> dsl-lint --dsl DSL` without a full path.
  Version-alignment guarantee: tools built from exactly the engine
  version they'll be linting against. Extra layer weight ~15 MB.
  `publish.yml` smoke test extended to invoke `dsl-lint --help` and
  `dsl-test --help` on both linux/amd64 and linux/arm64 (via QEMU)
  before cosign signs. New "Lint / test your DSL tree in CI" section
  in `README.md`.

### Fixed

- **Issue #85 — `template:` step no longer sends the string `"null"`
  as a header value.** A template step's `headers:` map whose value
  evaluated to `undefined` (e.g. `${incoming.headers['no-such-header']}`)
  used to pass the child DSL `incoming.headers.<name> = "null"` — the
  four-byte string — because the child_headers construction fell
  through `other.to_string()` for `Value::Null`. Downstream, any
  http step forwarding that value would send `X-Foo: null` on the
  wire. Fixed by filtering `Value::Null` out of the child headers
  map before stringifying, matching what `http_client` (issue #57)
  and `return_step` already do at their outbound seams. Tests:
  `null_valued_template_header_is_omitted_not_stringified`,
  `non_null_template_headers_still_forward`,
  `non_string_non_null_template_headers_stringify`,
  `null_header_dropped_alongside_other_kept_headers`.

## [0.9.13-rc] - 2026-09-09

Two fixes on top of v0.9.12-rc: issue #79 (stack-overflow abort when a
guard's `template:` step targets a resource under the same guard) and
PR #80 (`dsl-lint` rejected valid `ws_tag:` steps). Release-gate green:
533 passed / 0 failed / 3 ignored across 66 test binaries, dsl-lint
clean, dsl-test 100/100, cargo audit 0 warnings, mdbook builds.

**Behaviour change surface (grep before upgrading):**

- Guards that contain a `template:` step whose target is covered by
  the same guard no longer crash the process. The template step now
  filters the target's guard chain against a per-request "guards
  currently executing" stack; the same-key guard is skipped, breaking
  the recursion. If any DSL relied on the pre-fix crash as an
  accidental circuit-breaker (it can't have — the process aborted),
  it now returns a real response.
- A hard `MAX_GUARD_DEPTH = 32` cap surfaces exotic mutual-recursion
  patterns (three-guard cycles) as `RuuterError::DslExecution { step:
  "guard", … }` instead of a stack overflow. Legitimate nested-template
  compositions are nowhere near 32 deep.
- `dsl-lint` now accepts `ws_tag:` as a step primitive. Downstream CI
  pipelines that ran `dsl-lint` on DSLs using `ws_tag:` and grepped
  for a clean exit will now pass.

### Fixed

- **`dsl-lint` rejected valid `ws_tag:` steps.** `KNOWN_STEP_KEYS` in
  `src/bin/dsl_lint.rs` was never updated when the `ws_tag` step landed
  (0.9.8-rc, issue #52), so `dsl-lint` reported
  `step '<name>': unrecognised step` and exited 1 on any DSL that stamps
  a connection tag — even though the runtime parser
  (`src/dsl/parser.rs` `ACTION_KEYS`) accepts it. Added `"ws_tag"` to the
  linter's key list; the two lists now match.

- **Issue #79 — `template:` inside a guard no longer stack-overflows.**
  Reporter (sviljus) hit a fatal-abort regression in v0.9.11-rc / v0.9.12-rc:
  a project-wide `.guard.yml` that delegates its auth check to a
  `template:` step whose target is under the same guard would recurse
  forever. The chain was `HTTP entry → run guard → template → target's
  applicable_guards → same guard → template → …` — no cycle break
  anywhere, so the tokio worker aborted with `fatal runtime error:
  stack overflow` and the container exited 134. Introduced by v0.9.11-rc
  H1 (PR #72), where the template step started enforcing guards on its
  target for the first time; the recursion was inherent to that design
  but the cycle detector never landed.

  Fix threads a `guard_stack: Arc<Mutex<Vec<String>>>` through
  `ExecutionContext`. All three guard-loop call sites (HTTP entry,
  WS upgrade, template step) now:
  1. **Skip** any guard whose key is already on the stack (breaks
     the reporter's cycle at step 3 above).
  2. **Push** the guard's key before calling `engine.run(&guard, …)`
     and pop via an RAII `GuardStackGuard` drop-guard (works across
     the `>= 400` short-circuit and step-error return paths).
  3. **Cap** nesting at `MAX_GUARD_DEPTH = 32` — belt-and-braces
     against exotic mutual-recursion patterns (three guards
     circling) that slip past the same-key check. Hitting the cap
     surfaces as `RuuterError::DslExecution { step: "guard", … }`
     with a diagnostic naming every key on the stack and citing
     issue #79, so a DSL author sees the guard chain instead of a
     bare 500.

  The template step's child context now propagates the parent's
  stack Arc via `ExecutionContext::with_guard_stack_from` — the
  child context is a fresh `ExecutionContext::with_state` (not a
  clone), so the propagation is explicit.

  **Not a breaking change** for any correctly-shaped DSL. The pre-fix
  crash meant no operator could have shipped this shape in production.
  The one behaviour delta: if a guard's DSL previously invoked a
  `template:` step whose target's guards would have re-run the SAME
  guard, that redundant re-run no longer happens. The redundant run
  was a bug (H1 semantics = "check on the child context"; the check
  is already active from the enclosing invocation).

  Docs: `book/src/dsl/steps/template.md` and `book/src/dsl/guards.md`
  gained a "recursion / cycles" paragraph pointing at the guard-stack
  behaviour and the `MAX_GUARD_DEPTH` cap.

  Tests: `tests/issue_79_guard_template_recursion.rs` — 11 cases
  covering the reporter's minimal repro, the H1-preservation case
  (non-guard DSL still triggers target guards), a two-guard
  interleave, and RAII / cycle / cap unit tests on `push_guard`.

## [0.9.12-rc] - 2026-09-08

Issue #75 (sviljus / kemit-ee/efti-gate-ee) — full `declaration.allowlist`
contract fix. Four coupled bugs and two feature gaps in the
declaration-block enforcement path. Landed in PR #76 as one bundle;
release-gate green (522 passed / 0 failed / 3 ignored across 65 test
binaries).

**Behaviour change surface (grep before upgrading):**

- Guards now run BEFORE `allowlist:` stripping. Adding
  `declaration.allowlist.headers` to a route no longer silently
  strips headers the parent guard reads. Any DSL that relied on the
  pre-fix "guard sees stripped headers" behaviour (there shouldn't
  be any — that was the reporter's example A bug) will now see the
  raw wire request in the guard.
- Missing `required: true` field returns `400 Bad Request` (was
  `500`). Response body shape unchanged: `{"error": "Field missing:
  X"}`. If a client-side test asserted `status == 500` on this path,
  it needs to flip to `400`.
- `required: false` on structured `allowlist.body:` entries is now
  honoured. Pre-fix, every listed field was mandatory regardless of
  the flag; if you were sending a "kitchen-sink" body to work around
  that, you can drop the padding.
- Body `type:` mismatch on structured allowlist entries returns
  `400`. If any DSL declared `type: string` for OpenAPI purposes
  while accepting non-string values at the wire, callers now see a
  `400 Field type mismatch` instead of a `200` — declare the
  correct type, or leave `type:` unset for permissive behaviour.

### Added

- **Issue #75 — guard declarations are now enforced.** A guard's
  `declaration:` block is no longer inert. Before the guard's steps
  run, the router enforces (against the raw request):
  `required: true` fields, `required_one_of` groups, and body `type:`
  mismatches. Filtering (`strict:` / `additive:`) is a no-op on
  guards — guards check, they don't reshape the request for
  downstream (only the terminal DSL's declaration filters
  `incoming.*`). Existing guards that carry only
  `override_ancestors: true` are unaffected. Tests:
  `guard_required_one_of_all_missing_returns_400`,
  `guard_required_one_of_first_present_admits`,
  `guard_declaration_missing_required_returns_400`,
  `guard_declaration_type_check_enforced`,
  `guard_declaration_does_not_strip_undeclared_headers`,
  `guard_declaration_with_only_override_ancestors_still_works`.

- **Issue #75 — `allowlist.required_one_of` for OR-of-alternatives
  contracts.** Per-section (body / params / headers) groups; a group
  is satisfied when the request carries at least one of its members.
  Multiple groups AND together. Motivating case (issue example B):
  a guard that admits on `X-Api-Key` OR `X-Internal-Service-Token`
  can now declare its credential contract for OpenAPI consumers
  without turning both into "required" via the base allowlist.
  Diagnostic on miss: `Missing required_one_of in <section>: at
  least one of [x, y] must be present`. Works on both terminal DSLs
  and guards. Docs: new `required_one_of` section in
  `book/src/dsl/steps/declaration.md`. Tests:
  `terminal_dsl_required_one_of_all_missing_returns_400`,
  `terminal_dsl_required_one_of_first_present_succeeds`,
  `terminal_dsl_required_one_of_second_present_succeeds`,
  `terminal_dsl_required_one_of_body_group`,
  `terminal_dsl_multiple_required_one_of_groups_are_conjoined`.

- **Issue #75 — body `type:` is enforced at the wire.**

- **Issue #75 — guard declarations are now enforced.** A guard's
  `declaration:` block is no longer inert. Before the guard's steps
  run, the router enforces (against the raw request):
  `required: true` fields, `required_one_of` groups, and body `type:`
  mismatches. Filtering (`strict:` / `additive:`) is a no-op on
  guards — guards check, they don't reshape the request for
  downstream (only the terminal DSL's declaration filters
  `incoming.*`). Existing guards that carry only
  `override_ancestors: true` are unaffected. Tests:
  `guard_required_one_of_all_missing_returns_400`,
  `guard_required_one_of_first_present_admits`,
  `guard_declaration_missing_required_returns_400`,
  `guard_declaration_type_check_enforced`,
  `guard_declaration_does_not_strip_undeclared_headers`,
  `guard_declaration_with_only_override_ancestors_still_works`.

- **Issue #75 — `allowlist.required_one_of` for OR-of-alternatives
  contracts.** Per-section (body / params / headers) groups; a group
  is satisfied when the request carries at least one of its members.
  Multiple groups AND together. Motivating case (issue example B):
  a guard that admits on `X-Api-Key` OR `X-Internal-Service-Token`
  can now declare its credential contract for OpenAPI consumers
  without turning both into "required" via the base allowlist.
  Diagnostic on miss: `Missing required_one_of in <section>: at
  least one of [x, y] must be present`. Works on both terminal DSLs
  and guards. Docs: new `required_one_of` section in
  `book/src/dsl/steps/declaration.md`. Tests:
  `terminal_dsl_required_one_of_all_missing_returns_400`,
  `terminal_dsl_required_one_of_first_present_succeeds`,
  `terminal_dsl_required_one_of_second_present_succeeds`,
  `terminal_dsl_required_one_of_body_group`,
  `terminal_dsl_multiple_required_one_of_groups_are_conjoined`.

- **Issue #75 — body `type:` is enforced at the wire.** Structured
  `allowlist.body:` entries with `type:` now cause a `400 Bad Request`
  when the JSON body value's type doesn't match the declared type
  (`{"error": "Field type mismatch in body: <field> expected <declared>,
  got <received>"}`). Fixes row 3 of the reporter's table — pre-fix,
  a `type: string` receiving `123` silently succeeded because the
  runtime never consulted `field_type` (only the OpenAPI generator
  did). Skips null values (treated as absence), skips fields without
  a `type:` set, and skips unknown type names (forward-compat with
  OpenAPI vocabulary additions). Integer accepts JSON numbers with no
  fractional part (`42`, `42.0`); fractional numbers (`3.14`) fail.
  Params / headers are string-typed at the wire and are not enforced
  — that would need a separate coercion story. Docs: updated in
  `book/src/dsl/steps/declaration.md#per-field-metadata`. Tests:
  `body_string_field_receiving_number_is_400`,
  `body_integer_field_receiving_integer_is_200`,
  `body_integer_field_accepts_whole_number_float`,
  `body_integer_field_receiving_fractional_number_is_400`,
  `body_type_check_covers_all_primitive_types`,
  `body_field_without_declared_type_skips_check`,
  `legacy_flat_allowed_body_skips_type_check`,
  `body_unknown_declared_type_is_not_enforced`,
  `body_null_value_skips_type_check`.

- **Issue #75 — `declaration.additive: true` posture.** Third posture
  flag alongside `strict:`. When `additive: true`, the router does NOT
  filter body / params / headers down to the declared allowlist —
  undeclared fields pass through to `${incoming.*}` unchanged. The
  `required:` check still fires; OpenAPI still emits the declared
  schema. Use when the allowlist is documentation metadata only (the
  route legitimately consumes correlation headers or log-forwarded
  body keys it hasn't enumerated). Mutually exclusive with `strict:`;
  setting both is a parse-time error. Docs:
  `book/src/dsl/steps/declaration.md#additive`. Tests:
  `additive_body_passes_through_undeclared_fields`,
  `additive_headers_pass_through_undeclared`,
  `additive_still_enforces_required_fields`,
  `strict_and_additive_together_is_a_parse_error`.

### Fixed

- **Issue #75 — `declaration.allowlist` contract fixes.** Four coupled
  bugs in the declaration-block enforcement path, all reported by
  sviljus against `turnerrainer/ruuter:0.9.10-rc`:

  - **Guards now run BEFORE `allowlist` stripping.** Pre-fix the
    router filtered `incoming.body / .params / .headers` down to the
    route's `allowlist:` **before** dispatching the guard chain. A
    route whose `allowlist.headers` omitted a header its guard read
    (e.g. `xroad/.guard.yml` reading `X-Road-Id`) would silently
    break the guard — the guard saw the stripped view and 4xx'd every
    request (issue example A, 8 CI tests red). Post-fix, guards run
    against the raw wire request; only the terminal DSL sees the
    filtered view. The reorder is a security-adjacent fix: it closes
    a class of "adding a route-level declaration breaks the parent
    guard" regressions. New helper: `ExecutionContext::replace_request_view`
    on `src/context/mod.rs`. Test:
    `guard_sees_headers_not_listed_in_routes_allowlist`.

  - **`required: false` on structured `allowlist.body / .params:`
    entries is now honoured.** Pre-fix, every listed field was
    presence-enforced regardless of the flag — the runtime path
    collapsed `Vec<DslField>` down to `Vec<String>` of names and lost
    the metadata (issue example C, `dev-login.yml` couldn't declare
    `firstName` as optional; `admin/POST/v1/gates.yml` couldn't
    declare `tlsCert` as optional). Post-fix, the runtime matches the
    OpenAPI generator: default `false`; a field is only required when
    `required: true` is explicit. Legacy flat `allowed_body: [...]`
    is unchanged (no metadata slot → all listed fields required, as
    before). New helper: `required_body_field_names` on
    `src/router/mod.rs`. Tests:
    `structured_required_false_allows_missing_field`,
    `structured_required_absent_defaults_to_not_required`,
    `legacy_flat_allowed_body_still_presence_enforced`.

  - **Missing required field → `400 Bad Request`, not `500`.** The
    pre-fix `RuuterError::DslExecution { step: "declare", ... }`
    mapped to 500 — misleading (it's a client contract violation,
    not a server error) and useless for RFC-7807-style client
    tooling. Post-fix uses `RuuterError::BadRequest`, the same
    variant `declaration.strict: true` already returns for unknown
    keys. Response shape is unchanged (`{"error": "Field missing: X"}`).
    Test: `structured_required_true_missing_field_returns_400_not_500`.

  - **Doc fixes.** `src/main.rs` boot log now points at
    `book/src/dsl/steps/declaration.md` (correct path — was
    `book/src/dsl/declaration.md` and 404'd). `declaration.md` updated
    to describe the corrected `required` and status-code semantics.

  DSL authors: no change required for correctly-shaped DSLs. If you
  were working around the 500 by putting a `validate_input:` switch
  step ahead of the declaration, the switch step is now redundant for
  presence checks — the declaration returns 400 with the same
  `{"error": "..."}` shape your handler was returning. If you were
  omitting `allowlist.headers` because it broke your parent guard,
  you can now declare it safely; the guard runs on the raw request
  and only the terminal DSL sees the filtered view.

## [0.9.11-rc] - 2026-09-04

Security-hardening pass surfaced by an h2ck.me audit of the
v0.9.10-rc dev branch. Five findings landed as one bundle, plus
matching regression tests under `tests/security_h2ck_v0_9_10_rc.rs`
and updates to the two `security_hardening.rs` cases that pinned
the pre-fix contract.

### Security

- **H1 — `template:` step now enforces guards on the target DSL.**
  Pre-fix, a public DSL that said `template: admin/things` invoked
  the target route in-process without running the guards that gate
  it on the HTTP entry path (`POST/admin/.guard.yml` or a
  project-level `*` guard). Any operator who assumed "guarded on
  HTTP means guarded end-to-end" had a bypass. Fix: the template
  step calls `StepEngine::applicable_guards_for(project, dsl_key)`
  against a shared handle to the guard tree (`with_guards()` on the
  engine, wired from `main.rs`) and runs every applicable guard
  against the child context before dispatching the callee body. A
  guard returning `>= 400` short-circuits: the caller's `${result}`
  binds the guard's response, the target body never runs. Guards
  see the CHILD context, so the caller must forward auth headers
  explicitly via `template.headers` — mirroring what an HTTP call
  from the same caller would look like. Test:
  `template_step_must_run_target_dsl_guards`.

- **H2 — WebSocket upgrades now run guards; `/_/unguarded` reports
  WS routes.** Pre-fix, `handle_ws_upgrade` never called
  `applicable_guards`, and `audit_all_routes` explicitly filtered
  the `WS/` bucket out of its output. Operators who read the audit
  endpoint as "the complete list of routes with no guard" got a
  `unguarded: 0` verdict while an unauthenticated attacker could
  upgrade to any WS handler and start firing frames past the
  guard. Fix: the router runs the same guard chain on the WS
  upgrade path that HTTP requests get, against a synthesized
  `ExecutionContext` carrying the handshake headers + query
  params. A guard returning `>= 400` rejects the upgrade with that
  status; the client sees an HTTP-level failure and never gets a
  WebSocket. `audit_all_routes` now includes WS routes. Tests:
  `unguarded_audit_must_list_ws_routes_without_guards`,
  updated `audit_includes_ws_inbound_handlers`.

- **M1 — `GET /_/openapi.json` moved behind the admin gate.**
  Pre-fix, the auto-generated OpenAPI spec was mounted on the
  public router and enumerated every DSL route + declared
  request/response schema to unauthenticated callers. The
  pre-fix rationale ("describes the same DSL any client can probe
  anyway") stopped holding once `declaration:` blocks with typed
  schemas landed — the spec now surfaces declared PII field names
  and typed request shapes that an internal admin API has no
  reason to leak. Fix: `/_/openapi.json` mounts on `admin_router()`
  alongside `/_/sources` and `/_/unguarded`, so it's only reachable
  when the operator sets `RUUTER_ADMIN_ENABLED=true` (which they
  typically pair with reverse-proxy auth in front of `/_/*`). Tests:
  `openapi_json_must_be_admin_gated_by_default`, flipped
  `openapi_spec_admin_gated_by_default` in `security_hardening.rs`.

- **M2 — `RUUTER_HTTP_REWRITE` env in a release build now logs a
  WARN at boot.** The env var is documented as test-only but the
  code path was compiled into every release binary and consulted
  BEFORE `check_ssrf`. A stray setting in prod silently disabled
  SSRF allowlists and `block_private_networks` for the rewritten
  origin. Fix: new `rewrite_env_is_active_in_release()` helper on
  `http_client` returns true when the env var is set AND the
  current build has `debug_assertions` disabled. `main.rs` calls it
  at boot and emits a WARN in the same log stream as "Loaded
  config from …", so a misconfiguration is visible before the
  first outbound request fires. Test:
  `ruuter_http_rewrite_env_is_flagged_as_dangerous_in_release`
  (asserts the API exists).

- **M3 — WS outbound writer channels bounded (default 256).**
  Pre-fix, every WS registration used `mpsc::unbounded_channel`. A
  slow / dead reader combined with a `broadcast_where` fan-out
  could grow the sender queue without limit — memory-DoS surface.
  Fix: `Outbound` senders are `mpsc::Sender<Outbound>` with a
  bounded capacity (`DEFAULT_OUTBOUND_QUEUE_CAPACITY = 256`),
  helper `ws::bounded_sender(cap)` for callers. `WsRegistry::send`
  returns a concrete error naming the connection when the queue is
  full instead of buffering unbounded; `broadcast` /
  `broadcast_where` skip full queues so a slow subscriber can't
  stall fan-out to fast ones. Test:
  `ws_registry_send_must_be_bounded`.

### Contract changes

- Every DSL that reaches a target DSL via `template:` and expects
  to bypass the target's guards will now be rejected by that
  guard. Migration: forward the required auth headers explicitly
  via `template.headers: { authorization: "${incoming.headers.authorization}" }`,
  OR restructure so the shared logic lives in a non-guarded
  template (e.g. `templates/shared/…`) that the guarded DSL calls.
- Every WS DSL that lives under a project with a `.guard.yml`
  will now run that guard on connect. Migration: verify the guard
  DSL reads the handshake context (`incoming.headers`,
  `incoming.params`) rather than a request body (WS handshake has
  none) and that the auth check succeeds against the WS client's
  connect headers.
- `/_/openapi.json` no longer serves on the public router. If a
  caller relied on the public-router path, they must now hit
  `admin_router()` (i.e. run Ruuter with `RUUTER_ADMIN_ENABLED=true`
  and put their own auth in front of `/_/*`).
- `WsRegistry::send` returns `Err` when the peer's queue is full.
  DSL authors who assume `ws_send` always succeeds should either
  handle the error (`error:` branch) or expect the framework to
  log the failure and continue.

## [0.9.10-rc] - 2026-09-04

Ships five DSL / scripting / HTTP-client bug fixes surfaced by
@angryziber against v0.9.9-rc: PR #65 (issue #56), PR #66 (issue
#64), PR #67 (issue #61), PR #68 (issue #62), PR #69 (issue #63).

Pull:

```bash
docker pull turnerrainer/ruuter:0.9.10-rc     # pinned digest
docker pull turnerrainer/ruuter:rc            # moving :rc tag
```

Both registries are populated (Docker Hub + GHCR), both arches
(linux/amd64 + linux/arm64), all digests cosign-signed keyless.
`ghcr.io/turnerrainer/ruuter:0.9.10-rc` mirrors the Docker Hub
digest exactly.

Two of the five change observable behaviour in ways that could
affect existing DSLs — call out in ops notes:
- **#56** rejects any step with more than one action key at DSL
  load time. Deployments carrying previously-silently-mutated
  multi-key steps (e.g. `log:` alongside `call:`) will now refuse
  to boot with a clear error naming both keys. Fix or split.
- **#63** binds an empty upstream response body as `""` instead of
  `null`. DSLs that did `${result.response.body === null}` for
  "empty response" checks will flip; `${result.response.body}` on
  its own still reads falsy in both cases. Use
  `${result.response.body.length === 0}` to lock down the intent.

### Fixed

- **#56 — `log:` accepts a map; multi-action-key steps rejected at
  parse time.** Reporter's two asks in one ticket. First,
  `LogStep.log` widened from `String` to `serde_json::Value` so a
  DSL author can write `log: { user: "${who}", action: "login" }`
  and have every string leaf run through the script engine the
  same way `assign:` / `template.body:` / `http.args.body:`
  already do. Second, the parser used to dispatch by if-ladder
  priority (`call:` beats `template:` beats `assign:` beats …
  beats `log:`) and let serde silently drop every non-winning key
  — so a step with both `log:` and `call:` ran the HTTP call and
  discarded the log message from memory with no signal. Now:
  multi-action-key steps are a hard load-time error naming every
  offending key with the exact YAML syntax the author wrote and
  the full list of the 11 valid actions inline in the error. Both
  bugs are load-time (boot / hot-reload), zero per-request cost;
  well-formed DSLs unaffected. Docs: `book/src/dsl/steps/index.md`
  (One action per step section), `book/src/dsl/steps/log.md` (Map
  form section). Tests: `tests/audit_parser_dispatch.rs` (4 new
  multi-discriminator cases), `tests/logging_step_executions.rs`
  (`log_step_accepts_map_form`).

- **#64 — `switch:` conditions match on JS-truthy, not strict
  boolean.** `${requestId && poll}` returns the second operand
  (JS short-circuit), not `true` — so under the previous strict-
  boolean check (Java-parity `Boolean.TRUE.equals(...)`) the
  branch would be skipped when both operands were truthy strings,
  forcing DSL authors into `${!!requestId && !!poll}`. Now: the
  executor uses an `is_truthy` helper mirroring ECMAScript §7.1.5
  ToBoolean (`null` / `undefined` / `false` / `0` / `NaN` / `""`
  are falsy, everything else including `[]` and `{}` is truthy).
  Deliberate divergence from Java Ruuter — the expression language
  IS JavaScript, so the switch semantics should agree with what
  the surrounding language returns. Documented inline in
  `book/src/dsl/steps/switch.md` and (internal)
  `DIVERGENCES.md` D-40. Well-formed DSLs (`${age >= 18}`,
  `${a === 'buy'}`) still evaluate to real boolean `true` and
  are unaffected. Tests: `tests/switch_truthy_conditions.rs` (11
  cases including reporter's exact header-driven flow), unit
  tests in `src/steps/switch.rs::truthy_tests` (5), verified on
  both Boa and QuickJS backends.

- **#61 — `next:` to a non-existent step raises a runtime error
  naming source + target.** Previously the engine silently broke
  out of its execution loop when `next:` named a step not
  declared in the DSL, returning an empty 200 to the caller.
  Typos (`next: rply` instead of `next: reply`) and stale
  references (a step renamed but not every caller updated)
  surfaced as "empty response" mysteries in production. Now the
  engine raises a `DslExecution` error at the jump site: *"step
  '<source>' jumped to `next: <target>`, but no step named
  '<target>' exists in this DSL. Fix the target name, add the
  step, or use `next: end` to terminate."* Applies to every named
  jump target — top-level `next:`, `switch` branch `next:`,
  `http` `error:` fallback. The `next: end` reserved terminator
  is untouched. Runtime check (not parse-time) — O(1) hash
  lookup per jump, zero cost on the happy path. Tests:
  `tests/next_target_not_found.rs` (8 cases including chained
  jumps that must blame the immediate jumper, HTTP `error:`
  branches via mockito, case-sensitivity, `next: end` regression).

- **#62 — `??` and `?.` fire correctly on undeclared identifiers.**
  Issue #57 taught the engine to treat undeclared identifiers as
  `undefined` via a JS-level try/catch that returned undefined on
  ReferenceError. That wrap caught the error at the OUTERMOST
  layer, collapsing the whole expression to undefined and
  preventing `??` and `?.` from ever seeing the undeclared
  identifier as `undefined`: `${missing_var?.blah ?? '123'}`
  returned `null` instead of `'123'`. Fixed by retry-with-
  declaration in Rust: on ReferenceError, extract the identifier
  name from the engine's error message, declare
  `globalThis[X] = undefined`, retry. The retry then evaluates
  `undefined?.blah ?? '123'` under standard JS semantics and
  returns `'123'`. Bounded by `MAX_UNDECLARED_RETRIES = 16` so a
  legitimately broken expression can't loop unbounded. Preserves
  every #57 guarantee: bare undeclared → undefined; `?.` on
  undeclared → null; TypeError on declared-but-null.foo still
  surfaces (real DSL bug); mixed-string interpolation of null
  still renders as empty. Both engines (Boa + QuickJS). For
  QuickJS, task 045's registered-function fast path is preserved:
  the compiled function body is a bare `return (<expr>);` and
  the invoke wrapper catches in JS to hand `{__ok, v}` or
  `{__ok:false, name, message}` back to Rust, avoiding a fight
  with rquickjs's error-inspection surface. Docs:
  `book/src/dsl/js-gotchas.md` (the earlier "`??` doesn't fire on
  undeclared" caveat is now the "`??` DOES fire, no assign
  needed" note). Tests: `tests/nullish_coalescing_62.rs` (15
  cases across both engines: reporter's exact case, `||` vs `??`
  distinction on 0, mixed-string interpolation, chained `??`,
  retry cap ceiling, declared-undefined regression), unit tests
  in `src/scripting/mod.rs::extract_ident_tests` (5).

- **#63 — Empty upstream body binds as `""`; `Value::Null`
  forwarded as plaintext body is empty on the wire.** Two
  related bugs in one ticket. First: `http.<verb>` used to bind
  an empty upstream response body as `None` → `${result.response.body}`
  surfaced as JSON `null`. Now: empty body binds as
  `Value::String("")`, matching Java Ruuter and the wire truth.
  Second: a `Value::Null` body under `content_type: "plaintext"`
  used to serialise as the literal four bytes `n-u-l-l` on the
  outbound wire (via `serde_json::to_string(Value::Null)`).
  Now: null under plaintext → empty string on the wire.
  Defensive: fix (a) prevents #63's specific null source, but
  the same footgun could bite from anywhere else a null slips
  into the outbound plaintext body slot. Docs:
  `book/src/dsl/steps/http.md` (Empty body → "" note replaces
  the earlier "Empty body → null"). Tests:
  `tests/empty_response_body_63.rs` (9 cases: empty text/plain
  and application/json bind as string, `Value::Null` forwarded
  as plaintext arrives empty, 3-hop chain, whitespace-only body
  binds verbatim, `.length` behaviour-change lock-down).

## [0.9.9-rc] - 2026-09-03

Ships PR #59 (issue #57) and PR #58 (issue #54): undeclared
identifiers in `${…}` now evaluate to `undefined` (making template
composition tractable), `null`/`undefined` no longer surface as the
literal string `"null"` in headers/query params/mixed-string
interpolation, and the switch no-match log field renamed
`condition=undefined` → `condition=no_match`. Doc-only PR #55
brought three stale pages in line with `logging.format: pretty`
along the way.

### Fixed

- **#57 — Undeclared identifiers in `${…}` no longer fail the DSL;
  `null` / `undefined` no longer surface as literal `"null"` on the
  wire.** Two independent surprises reported in the same issue,
  fixed together. **First**, `${platform?.id}` where `platform` was
  never bound used to throw `ReferenceError: platform is not defined`
  and 500 the request — even though `?.` is exactly the syntax JS
  provides for "safe read of a possibly-missing thing." Composition
  of DSL templates (a caller passes a subset of the fields the
  callee might consult) was effectively impossible. Both scripting
  backends (Boa default, QuickJS optional) now wrap each `${…}` in
  a try/catch that swallows *only* `ReferenceError` — undeclared
  identifiers evaluate to `undefined` (→ JSON `null`), while
  TypeError from `foo.bar` on a declared-but-null `foo` still
  surfaces, because that is a real DSL bug and `?.` is the tool for
  it. **Second**, deep-dive-hunted the six sibling sites where a
  script-evaluated `Value::Null` was rendered as the string
  `"null"`: response headers (`return` step), outbound TCP request
  headers + query params (`http_client/mod.rs`), outbound UDS
  headers (`uds.rs`, `uds_pool.rs`), mixed-string interpolation
  (both engines). All now drop the header/param entirely (HTTP has
  no null-valued header) or interpolate as empty (`"hi ${absent}"`
  → `"hi "`, not `"hi null"`). Body semantics unchanged and
  already spec-compliant per `undefined_in_object.rs`: `undefined`
  drops from object properties, both become JSON `null` in array
  slots. Regression suite: `tests/undefined_identifier_57.rs`
  (9 tests, engine contract + end-to-end). Two prior tests
  (`error_response_details`, `single_flight_step`) that used
  undeclared identifiers to *trigger* a JS error switched to
  `${(null).some_field}` — the diagnostic contracts they cover are
  unchanged.

### Changed

- **#54 — Switch no-match log value renamed `undefined` → `no_match`.**
  On a `switch` step whose conditions all evaluate false, the
  per-step `Executed` INFO line now reads
  `▸ <step> (switch) … → <next> condition=no_match` instead of
  `condition=undefined`. Field name (`condition=`) is unchanged, so a
  single grep predicate still catches both branches
  (`condition=0`, `condition=1`, …, `condition=no_match`). The
  earlier value (`undefined`) came from #37, chosen for JS-native
  symmetry; the reporter of #54 read the log and could not tell that
  it meant "no branch matched, fell through to `next:`" — the
  snake_case rename brings the value in line with the rest of
  Ruuter's log-attr casing (`terminated_by=end_of_steps`,
  `dsl.steps_ran`, …) and reads unambiguously without JS knowledge.
  Dashboards / alerts filtering on the literal string
  `condition=undefined` need updating. Chosen path continues to
  appear in the `→ <next-step>` positional column on every switch
  line.

## [0.9.8-rc] - 2026-09-02

Ships PR #51 (issue #52) end-to-end: targeted WebSocket fan-out
without an external session directory. A WS server DSL can now
stamp identity tags on the originating connection and later
address broadcasts by tag match — filling the gap between
`broadcast_prefix` (over-delivers) and explicit `to:` lists
(needs the id set already known).

### Added

- **#52 — Connection tags for the WebSocket server: `ws_tag` step +
  `ws_send: { broadcast_where: … }`.** Until now a WS server DSL had
  two ways to fan a frame out — `broadcast_prefix` (every connection
  whose id starts with a prefix) or an explicit `to:` list of ids —
  and no way in between. Any "send this only to the connections that
  are allowed to see it" delivery forced the DSL author to stand up
  an external store mapping each `client:<hex>` id to a session, keep
  it fresh as sockets come and go, and consult it on every send.
    - **`ws_tag: { set: { <key>: <expr>, … } }`** — new step. Stamps
      string tags on the connection the current frame arrived on
      (`context.connection_id()`). Each value is script-evaluated and
      coerced to a string. Merges with existing tags; errors outside a
      WS DSL. The intended pattern: authenticate the handshake on the
      first frame, then `ws_tag` the identity you resolved (`user`,
      `roles`, `tenant`, …).
    - **`ws_send: { broadcast_where: { tag: "roles", contains:
      ",admin," } }`** — new addressing mode, priority above
      `broadcast_prefix` and `to:`. Fans out to exactly the
      connections whose tag matches. `equals` (whole value) and
      `contains` (substring) operands, both script-evaluated so
      `${…}` works. A connection without the tag never matches.
      Both operands and the tag key must resolve to a non-empty
      string; an empty `contains` would match every tagged connection
      and is almost always an unresolved `${…}`, so it's rejected
      outright.
    - Tags are process-local and dropped on unregister. No wire-format
      change, no new config. `WsRegistry` gains `set_tags`,
      `tags_of`, `broadcast_where`; the existing `broadcast` and
      `send` paths are untouched.
    - Docs: `docs/DSL_REFERENCE.md` §6.1. Tests: `tests/ws_server.rs`
      (`ws_tag_scopes_broadcast_where_to_matching_connections`) plus
      unit coverage in `src/ws/mod.rs`.

## [0.9.7-rc] - 2026-09-01

Ships PR #48 end-to-end: two safety nets for the "silent unguarded
route" trap surfaced by the #41 discussion. One goes into CI
(`dsl-lint --require-guard`), the other into runtime
(`GET /_/unguarded` admin endpoint). Same underlying audit helper —
lint and endpoint cannot disagree about which routes are guarded.

### Added

- **#45 — Guard-audit tooling: `dsl-lint --require-guard` +
  `GET /_/unguarded`.** Two safety nets for the "silent unguarded
  route" trap surfaced by the #41 discussion (sibling guards are
  name-scoped, not directory-scoped — a peer `.yml` file in the same
  folder as `foo.guard.yml` and `another.guard.yml` can end up
  unguarded by accident).
    - **`dsl-lint --require-guard`** — new opt-in flag. Loads the DSL
      tree via the same loader the runtime uses, walks every HTTP
      route through the shared audit helper, and emits one error per
      route with zero applicable guards. Exits non-zero when any
      unguarded route is found. Default off — public endpoints
      legitimately exist. Use in CI on projects with a "no unguarded
      routes ever" policy. HTTP routes only; WS/inbound is excluded
      because the guard chain doesn't fire on the WS path today.
    - **`GET /_/unguarded`** — new admin endpoint (gated by
      `RUUTER_ADMIN_ENABLED=true`, same as `/_/sources`). Runtime
      inventory of guarded vs unguarded routes across every loaded
      project. Guarded entries name the applicable guard keys in
      outer-first execution order (`*` = project-level guard, issue
      #39; `<METHOD>/<path>` = method-scoped). Totals at the top for
      dashboard panels. Deterministic sort order for meaningful
      cross-deploy diffs. Complements the lint — same underlying
      helper, so a route flagged by one is flagged by both.
    - **Refactored `DslRouter::applicable_guards` to delegate to the
      new shared helper** (`crate::dsl::guard_audit::guard_keys_for_dsl`).
      Single source of truth for guard-matching semantics — the hot-
      path resolver, the lint, and the admin endpoint cannot drift.
      21 existing guard tests (across `tests/guards.rs`,
      `tests/project_level_guard.rs`, `tests/sibling_guard_same_dir.rs`)
      pass unchanged, confirming the refactor is behaviour-preserving.
    - **7 integration tests** in `tests/guard_audit.rs` cover:
      guarded vs unguarded reporting, project-level key surfacing,
      stacking order, exact-match branch (#41 lock-in),
      override_ancestors bypass, `GuardMode::ClosestOnly` interaction
      with the project guard, WS/inbound exclusion.

## [0.9.6-rc] - 2026-09-01

Ships PR #44 end-to-end: fixes a silent security-shaped bug in the
sibling guard convention (issue #41). Patch-level RC bump because
this is a fix-only release with no new features.

### Fixed

- **#41 — Sibling guard silently skipped when guard and DSL share a
  directory.** A `<stem>.guard.yml` next to a `<stem>.yml` file in the
  same directory produced identical guard and DSL keys
  (`<METHOD>/path/<stem>`); `applicable_guards` did a trailing-slash
  prefix check (`starts_with("<METHOD>/path/<stem>/")`) which failed
  for the same-key case, silently skipping the guard and leaving the
  route unguarded. `applicable_guards` now accepts exact-match too, so
  a sibling guard covers both the same-name DSL AND every DSL under a
  same-name folder. The prefix branch still handles ancestor guards
  over child DSLs — no regression there. Security-shaped: any
  deployment relying on a sibling-same-directory guard for auth was
  previously unguarded and now correctly rejects unauthorised
  requests. 4 regression tests in `tests/sibling_guard_same_dir.rs`:
  the exact repro from the issue, prefix-match on children still
  works, one guard covers both same-key and children, and the
  peer-with-different-stem case remains correctly unguarded (locking
  in name-scoped-not-directory-scoped semantics).
- **Docs — sibling guard semantics.** `book/src/dsl/guards.md`
  expanded: the sibling convention section now covers the same-key
  case, the per-endpoint pattern (sibling with no matching folder),
  and an explicit "sibling guards are name-scoped, not directory-
  scoped" trap section with the exact `is_this_unguarded.yml` example
  from the discussion — plus a variant-precedence table for
  `.guard` / `.guard.yml` / `.guard.yaml`.

## [0.9.5-rc] - 2026-09-01

Ships PR #42 end-to-end: cross-method authorisation without per-method
guard duplication (issue #39). One `.guard.yml` at the project root
now protects every HTTP endpoint in the project. Removes the
copy-paste guard-file boilerplate that operators porting from Java
Ruuter kept hitting.

### Added

- **#39 — Project-level `.guard.yml`.** A single `<project>/.guard.yml`
  (or `.guard` / `.guard.yaml`) at the project root now applies to
  every HTTP method in the project. Removes the boilerplate of copying
  the same auth check into `GET/.guard.yml`, `POST/.guard.yml`,
  `PUT/.guard.yml`, and so on. Runs as the outermost guard: project →
  method-root → path-ancestor → target. Stacks with method-scoped
  guards; a nested guard with `declaration.override_ancestors: true`
  still replaces every ancestor including the project-level one — the
  escape hatch for a public endpoint under an otherwise-protected
  project remains intact. Stored under the reserved guard key `*`
  (a value no valid `<METHOD>/<path>` key can produce), so the change
  threads through the existing `SharedGuards` / hot-reload plumbing
  without a new type. `override_ancestors: true` on the project-level
  guard itself is meaningless (nothing outside it to override) — the
  loader WARNs and ignores the flag. Two conflicting variants at the
  project root (`.guard.yml` alongside `.guard.yaml`, etc.) is a
  load-time error naming both offending files rather than a silent
  fs-iteration-order pick. New runnable example under
  `DSL/guarded-demo/` demonstrates one guard protecting both a GET
  and a POST endpoint. Docs: new "Three file conventions" section in
  `book/src/dsl/guards.md`, plus a runnable-example walkthrough.
  5 integration tests in `tests/project_level_guard.rs` cover
  cross-method coverage, stacking with a method-scoped guard, the
  override bypass, no-guard sanity, and the two-file load-error.

## [0.9.4-rc] - 2026-08-31

Feature-and-polish release cycled on top of 0.9.0-rc.3. Ships PR #38
end-to-end: Java-parity per-step INFO trail (issue #37), a compact
terminal-first text formatter, a new opt-in `pretty` format for
interactive dev, and follow-up cleanups (duplicate `dsl log step`
event removed, per-step `attrs` field names shortened, `return` /
`state.set` / `template` gained content previews on the trail).

Version numbering: skips 0.9.0/1/2/3 (never cut as stable) in favour
of `0.9.4-rc` — pre-release marker for a target 0.9.4 stable rather
than continuing the `0.9.0-rc.N` counter.

### Fixed

- **Duplicate log-step line removed.** Every `log:` DSL step used to
  emit both a `dsl log step` INFO event with a `dsl.log=…` field
  AND the per-step `▸ … (log) …  msg="…"` Executed line — same
  message, two lines. The `dsl log step` event is gone; the
  interpolated message now rides only as `attrs.msg` on the
  `Executed` line. JSON consumers that keyed on `dsl.log` should
  read `attrs.msg` on the `Executed` event instead.

### Documentation

- **New `Recipes → Reading a live trail` section** in the mdbook
  ([`book/src/logging/recipes.md`](book/src/logging/recipes.md#reading-a-live-trail))
  with verbatim per-step-type output for every representative
  sample DSL: `GET /samples/ping`, `variables/assign-simple`,
  `things` (all three switch outcomes), `state/inc`,
  `advanced/logging-demo`, `advanced/iterate-batch`. Format
  examples in `formats.md` refreshed against the post-#37 short
  attrs field names. Link to be added on issue #37 when shipped.

### Changed

- **Compact one-line-per-event text formatter (terminal readability).**
  The default `text` format switches from `tracing_subscriber`'s
  built-in fmt layer to a custom formatter tuned for terminal
  reading. Every event fits one line on any terminal ≥ 120 cols:
  `HH:MM:SS.mmm LEVEL [t=<8hex> <project>] ▸ <step> (<type>)
  <duration> → <next>  <attrs>` for `Executed`, `HH:MM:SS.mmm
  LEVEL [t=<8hex> <project>] ⏹ <METHOD> <route> <status> <duration>
  from <ip>` for the access log. Span noise dropped from text
  rendering: `otel.name`, `http.request.method`, `http.route`,
  `client.address` no longer duplicate onto every child event
  (they remain on the OTLP span). Rust module target
  (`ruuter_on_rust::steps::engine`) dropped. Timestamps trimmed
  from nanosecond ISO-8601 to `HH:MM:SS.mmm` UTC.

### Added

- **`logging.format: pretty`** (env `RUUTER_LOG_FORMAT=pretty`).
  Same layout as `text` plus ANSI colours (level, step marker,
  duration, status) and Unicode markers (`▸` for step,
  `⏹` for access log). Intended for interactive local dev; do
  not pipe to files or aggregators (colour escapes leak).

### Fixed

- **`duration_ms` float-precision artefact.** Values like
  `0.05121800000000001` on the log line — an artefact of f64
  rendering — are gone. `crate::logging::duration_ms` now
  computes via `Duration::as_micros() as f64 / 1000.0`, giving
  microsecond precision (0.001 ms) without float tails.

- **Compact `attrs` field names.** The per-step `attrs=` on the
  `Executed` INFO line dropped its step-type prefix — the step type
  is already on the line (`(state)`, `(switch)`, `(return)`, …), so
  fields like `state.op="get"` are now just `op="get"`,
  `http.response.status_code=200` is `status=200`. Full OTel
  semantic-convention names still appear on the primary access log
  line and on the OTel span for dashboard portability.
  `switch.next=…` dropped entirely (redundant with the engine's
  `→ next-step` positional column). Switch attrs renamed:
  `switch.matched_branch=1` → `condition=1`, `switch.matched_condition="..."`
  → `expr="..."`; no-match case renamed `matched="no-match"` →
  `condition=undefined` for a single greppable predicate across
  both branches. Net effect: an `Executed` line for a state.get
  went from ~120 chars to ~95.

- **`return`, `state.set`, `template` steps now surface content
  on the trail.** Previously the `Executed` line for these
  "answer" / side-effect steps only showed status / key metadata,
  making the trail read like an access log without status codes.
  Added: `return.body` (capped + redacted 80-char JSON preview of
  the returned value), `state.value` (same treatment for the value
  written), `template.body` (same for the callee's return). Values
  honour `redact_body_fields` so project-specific PII / secret
  extensions apply. Uses a new
  `StepLogExtras::push_preformatted` variant so JSON previews
  render as `return.body={"counter":1}` rather than the double-
  quoted `return.body="{\"counter\":1}"`.


### Added

- **#37 — Java-parity per-step INFO execution trail.** Java
  Ruuter emitted one INFO `Executed: <step-name>` line per DSL
  step via `LoggingUtils.logStep()` at default log level; Ruuter-
  on-Rust only had a DEBUG-gated `step_timing` line, off by
  default. Result: at INFO (production) the DSL was a black box.
  Fix: engine now emits one INFO `Executed` line per step with
  `dsl.step`, `dsl.step.type`, `duration_ms`, `dsl.next.step`,
  and a rendered `attrs` field carrying step-type-specific
  context (HTTP: `url.full` + upstream status; switch: matched
  branch; return: response status; state: op + key + hit; log:
  message; iterate: item count; template: dsl + child status;
  assign: keys; ws_send: mode + delivered; single_flight: role +
  key; http_mock: status). Two new config knobs:
  `logging.log_step_executions` (default `true`, the Java-parity
  trail) and `logging.log_dsl_runs` (default `false`, opt-in
  Rust-only enrichment — the request span already brackets each
  run via `trace_id`, so the extra `DSL run started` /
  `DSL run completed` INFO lines are opt-in for grep-based triage
  where an explicit `terminated_by` label helps). Operators can
  drop `log_step_executions` for very high-QPS DSLs.
  Executor-side plumbing goes through a new
  `StepResult.log_extras: StepLogExtras`, order-preserving with
  CR/LF-sanitising Display so an attacker-controlled URL or log
  message can't splice a fake log line. Docs updated
  (`book/src/logging/configuration.md`, `fields.md`, and
  `java-parity.md`).

## [0.9.0-rc.3] - 2026-08-28

Bug-fix roll-up on top of 0.9.0-rc.2. All three fixes share a common
theme: shapes that were legal in Java Ruuter (and that ordinary DSL
authors reach for) either crashed the runtime or were rejected at DSL
load time. rc.3 makes them work as expected without changing existing
behaviour for the shapes that already worked.

### Fixed

- **#33 — Ruuter crashed on undefined input.** Any script expression
  that evaluated to an object with an `undefined` property panicked
  the tokio worker with `not yet implemented: undefined to JSON` from
  boa's built-in `JsValue::to_json`. The non-array object branch of
  `js_value_to_json` now routes through `JSON.stringify`, which per
  JS spec drops undefined properties from objects and turns undefined
  array slots into `null` — matches the QuickJS backend's existing
  behaviour. The serialisation slot is registered non-writable so a
  script can't hijack it mid-evaluation.

- **#34 — `Object.assign` with a missing header crashed the request.**
  The exact reproduction — `Object.assign(base, { 'x-request-id':
  incoming.headers['x-request-id'] })` when the source header is
  absent — is the practical trigger for #33. Same fix, same commit;
  end-to-end regression via the axum router (missing-header returns
  200 not 500; present-header still propagates).

- **#32 — Template step rejected `${expr}` for `body`, `query`,
  `headers` at DSL load time.** The strict `Option<HashMap<String,
  Value>>` typing meant a top-level `body: "${followup_json.response.body}"`
  failed with `invalid type: string "${...}", expected a map` before
  the DSL ever ran. Loosened to `Option<Value>` and evaluated at
  runtime via the shared `evaluate_map_arg` helper — exact parity
  with the 0.9.0-rc.1 #25 fix for `http.<verb>` and `return`.
  Non-object runtime results still surface as a clear diagnostic
  naming the step + arg.

  19 regression tests across `tests/undefined_in_object.rs` and
  `tests/template_dynamic_map_args.rs` — including composition tests
  that exercise `Object.assign` + `undefined` through the template
  step's body/query/headers.

## [0.9.0-rc.2] - 2026-08-26

Fast follow to 0.9.0-rc.1: same-day upstream fixes for two response-body
error-clarity issues (#28, #29). The 0.9.0-rc.1 logging chapter
enriched **log lines** with cause chains + step context, but the
**JSON error response** the API caller receives was still just the
top-level `Display`, discarding both the step context (#28) and the
`std::error::Error::source()` chain (#29). rc.2 wires those into the
response body — same enrichment shape as the log lines, uniformly.

### Fixed

- **#28 — No error details when a JavaScript expression fails.**
  Before: the response body was just the raw script engine error
  (e.g. `Script evaluation error: TypeError: cannot convert null
  or undefined to object`) with no indication of which DSL step
  ran the failing expression. Now: every step error is wrapped
  with `step '<name>' (<type>) in project '<project>' failed` at
  the engine boundary via a new `RuuterError::StepContext`
  variant. The response identifies the failing step + step type +
  project by name.

- **#29 — No error details when an outgoing HTTP request fails.**
  Before: the response body was the top-level reqwest Display
  only (e.g. `HTTP error: error sending request for url (...)`)
  — the actual cause (DNS failure, connection refused, TLS
  handshake, timeout) sat in `std::error::Error::source()` and
  never surfaced. Now: the router's error response builder walks
  the full source chain via `logging::error_chain()` (bounded to
  5 hops) and joins it as `-> caused by: X -> caused by: Y`.
  The caller sees the actual OS-level failure directly.

  Combined output for both fixes (real example from an
  unresolvable hostname):
  ```json
  {"error": "step 'call_upstream' (http) in project 'consignment' failed
             -> caused by: HTTP error: error sending request for url (...)
             -> caused by: error sending request for url (...)
             -> caused by: client error (Connect)
             -> caused by: dns error
             -> caused by: failed to lookup address information: Temporary failure in name resolution"}
  ```

  4 regression tests in `tests/error_response_details.rs`. Book
  chapter `book/src/logging/errors.md` documents the new
  response-body shape.

## [0.9.0-rc.1] - 2026-08-26

Substantial feature release: comprehensive structured-logging chapter,
task 070 declaration-parity-with-Resql, and four upstream fixes
(#23, #24, #25, #26). Version bumped to 0.9.0 (from 0.8.1-rc series)
per SemVer — the logging observability surface and the DSL
declaration richness are additive but large enough to warrant a
minor.

### Fixed

- **#24 — `return` with `wrapper: false` always JSON-serialised
  the body.** A DSL returning an XML/HTML/plaintext string with
  `Content-Type: text/xml` (or similar) still went through
  `axum::Json`, so the response body came out wrapped in double
  quotes with characters JSON-escaped. Fixed in
  `router/mod.rs`: when the DSL sets `wrapper: false`, the
  return value is a JSON string, AND the DSL declared a
  non-JSON Content-Type header, bypass `axum::Json` and emit
  the raw string bytes with the DSL's Content-Type. All other
  shapes (objects, arrays, numbers, DSLs without an explicit
  non-JSON Content-Type) stay on the JSON path so prior
  behaviour is preserved. 8 regression tests in
  `tests/non_json_response.rs` cover both this fix and #23.

- **#23 — `http.<verb>` step lost non-JSON upstream response bodies.**
  Upstream responses that weren't valid JSON silently became
  `null` in `${result.response.body}` — an XML mapper's
  `<root>…</root>`, an upstream's plain-text diagnostic, or any
  `text/*` payload was unrecoverable to the DSL. Fixed at
  `http_client/mod.rs`: try JSON parse first (unchanged for JSON
  responses), fall back to `Value::String(from_utf8_lossy(bytes))`
  on parse failure. Empty bodies still become `None`. Binary
  bytes are UTF-8-lossy-decoded (`U+FFFD` for invalid sequences)
  rather than panicking.

- **#25 — Cannot provide a dynamic headers map.** `http.<verb>`
  step's `headers:` and `query:` args (and `return` step's
  `headers:`) rejected a top-level `${expr}` string at DSL load
  time with `invalid type: string, expected a map`. The parser
  never handed the value to the script engine. Fixed by loosening
  the field type from `Option<HashMap<String, Value>>` to
  `Option<Value>` and evaluating both shapes at runtime:
  - **YAML mapping** (traditional) — each value evaluated per-key.
  - **`${expr}` string** — evaluated once; result MUST be a JSON
    object (else clear step error naming the field); `null` = no
    headers.

  Enables the merge-headers pattern from the issue:
  ```yaml
  merge_headers:
    assign:
      merged_headers: "${Object.assign({}, ...)}"
  forward:
    call: http.post
    args:
      headers: "${merged_headers}"    # now works
  ```

  New integration tests in `tests/dynamic_map_args.rs` cover
  parse-time (both shapes), runtime evaluation, and the
  non-object diagnostic path.

- **#26 — YAML parse failure did not name the file.** DSL loader
  errors bubbled up a bare `serde_yaml_ng::Error` (line + column
  only) rendered as `Failed to load DSLs: YAML error: did not find
  expected key at line 55 column 39, ...`. An operator with dozens
  of DSLs had no way to tell which file was broken. Fixed at
  `parser.rs::parse_file` — every error out of that boundary is
  now wrapped as `DSL parsing error: <path>: <underlying>`. The
  underlying YAML diagnostic (line, column, context) is preserved
  in full; the path just gets prepended. Regression tests in
  `tests/parse_error_file_path.rs`.

### Added

- **Declaration parity with Resql (task 070).** DSL `declaration:`
  block gains:
  - **Rich per-field metadata** — `DslField` now carries `type`,
    `required`, `format`, `description`, `default`, and (for arrays)
    `items`. Bare `{field: X}` entries continue to parse; richer
    shape is additive.
  - **Typed `returns:`** — structured response schema flows into
    OpenAPI 2xx response body.
  - **`strict: true` per-DSL posture** — unknown body / query /
    header keys return **400 Bad Request** with a diagnostic
    naming the field, instead of silently filtering. Traceparent
    is always allowed under strict headers (framework-injected).
  - **Boot-time WARN per HTTP DSL missing a declaration** — never
    fatal; the DSL still loads and runs. Gated by
    `dsl.warn_on_missing_declaration` (default `true`); flip to
    `false` in `ruuter.yaml` to silence for corpora that
    intentionally run permissive. Per operator instruction
    (2026-08-25): missing declaration NEVER halts Ruuter.
  - **New `RuuterError::BadRequest` variant** maps to 400 in the
    response builder (used today by the strict-key gate; extensible
    to other client-input rejections).
  - **Removed dead struct fields** `method` and `accepts` from
    `DeclarationStep`. Repurposed `returns` from an unread
    `Option<String>` to a typed `Option<Vec<DslField>>`. Old
    `returns: "<string>"` values are silently ignored (they were
    never read in prior Rust versions).
  - **New sample** `DSL/samples/POST/typed-users/create.yml`
    demonstrates the full richer shape.
  - **New book chapter** `book/src/dsl/steps/declaration.md`
    rewritten to cover the whole surface.
  - **12 new integration tests** in `tests/declaration_parity.rs`.
  - **New parser API** `DslParser::parse_content(&str)` so tests /
    linters / IDE plugins can parse in-memory DSLs without a
    filesystem path.
  - **New `openapi.rs` helpers** `field_schema`, `build_object_schema`,
    `build_named_parameter` for typed schema emission.
  - See `DIVERGENCES.md` D-39 for the full parity write-up.

- **Structured logging (industry-standard).** Full observability
  section at `book/src/logging/`. Every request opens a
  `tracing::info_span!("http_request", …)` carrying OpenTelemetry
  HTTP semantic-convention fields (`http.request.method`,
  `http.route`, `http.response.status_code`, `client.address`) plus
  DSL context (`dsl.project`, `trace_id`), so every log line inside
  a request is automatically decorated. One INFO access-log line
  per completed request. New `src/logging/` module handles
  redaction of secret-bearing headers and JSON body fields
  (case-insensitive, recursive), body caps, CRLF stripping
  (log-injection defence), and bounded error-chain rendering.
- **JSON log format.** `logging.format: json` (or env
  `RUUTER_LOG_FORMAT=json`) emits one OTel-log-shape JSON object
  per event. Default remains `text` for local dev.
- **Per-step DEBUG timing.** `logging.step_timing: true` emits a
  `dsl.step` / `dsl.step.type` / `duration_ms` DEBUG line per step
  (mirrors Java's `LoggingUtils.logStep`).
- **Outbound HTTP body dumps** (Java parity). `display_request_content`
  and `display_response_content` config flags — previously
  accepted-but-inert — are now wired end-to-end via
  `src/steps/http.rs`. Redacted and capped by the same knobs that
  guard access-log fields.
- **Structured error rendering.** `meaningful_errors: true` emits
  a second WARN line with the underlying `source().to_string()`;
  `print_stack_trace: true` includes the `source()` chain
  (bounded to 5 hops) on the primary ERROR line.
- **Trace-id lifecycle unified.** `handle_request` adopts inbound
  `traceparent` or generates one at request entry, injects it
  into the request headers so it's visible to every downstream
  step AND matches the `X-Trace-Id` returned in the response.
  Previous behaviour computed a fresh id at response-write time
  that didn't match the DSL-side value.

### Changed

- **`observability::init` signature** now takes `&AppConfig` (was
  no-arg) so `logging.format` is honoured at boot. `main.rs` now
  loads config before initialising the subscriber.
- **D-29** in `DIVERGENCES.md` expanded to cover structured logs.
  **D-35** shrunk — four `logging.*` fields wired end-to-end, no
  longer WARN at boot.
- **`book/src/config/inert-fields.md`, `book/src/ops/env.md`,
  `book/src/ops/configuration.md`** updated to match the wired
  behaviour and reference the new logging chapter.

## [0.8.1-rc.3] - 2026-08-05

Hotfix release. Re-cuts `0.8.1-rc.2` with the smoke-test regression
introduced by the wrapper-default flip. `0.8.1-rc.2` images are on
both registries but are **unsigned** — the smoke test in `publish.yml`
asserted `/samples/ping` returned `"pong"` while the flipped wrapper
default now returns `{"response":"pong"}`, so cosign never ran.
Pull `0.8.1-rc.3` for signed, smoke-verified images.

### Fixed

- **`.github/workflows/publish.yml`** — smoke test now expects the
  wrapped shape `{"response":"pong"}` on `/samples/ping`, matching
  the runtime behaviour under `response.default_wrapper: true`.
- **Book pages** documenting `/samples/ping` responses updated to the
  wrapped shape: `book/src/introduction.md`,
  `book/src/getting-started/run-locally.md`,
  `book/src/getting-started/postman.md`,
  `book/src/dsl/steps/return.md`.

### Notes

- Everything else from `0.8.1-rc.2` applies verbatim — this is only
  a smoke-test + docs fix. No runtime code change.
- Consumers who pulled `0.8.1-rc.2` should switch to `0.8.1-rc.3`
  before running cosign verification.

## [0.8.1-rc.2] - 2026-08-05

Second **pre-release** cut. Java-parity audit sweep (17 findings)
plus two behavioural changes on top and a source-of-truth parse
gate. Not GA. Publishes as `turnerrainer/ruuter:0.8.1-rc.2` on
Docker Hub and `ghcr.io/turnerrainer/ruuter:0.8.1-rc.2` on GHCR.

### Audit sweep (commits `f6b62f4`..`8698345`, 2026-08-04)

Seventeen Java-parity findings closed, each with a paired
regression test under `tests/audit_*.rs` (285 tests total across
the audit-regression + pre-existing suites).

- **01** — Step-driven `reload_dsl:true` (Java parity) alongside the
  filesystem watcher.
- **02** — Hot-reload watcher filters events by kind + path (loop fix).
- **03** — `BaseStepFields` (`skip`, `sleep`, `maxRecursions`,
  `reloadDsl` with aliases) flattened onto every step; engine honours
  each.
- **04** — `HttpStep.error` routes on non-allowed status.
- **05** — Set-Cookie hardening + nested header eval on `return:`.
- **06** — TemplateStep binds raw return value (not fake HTTP envelope).
- **07** — `incoming_requests.headers` injected on every request.
- **08** — Per-step `maxRecursions` cap.
- **09** — Explicit discriminator dispatch in `DslParser` (kills the
  typo-swallowing untagged-serde fallthrough).
- **10** — `declare:` allowlist enforced; structured `allowlist:` form.
- **11** — Multipart / form-encoded / text inbound + outbound
  content-type dispatch.
- **12** — Response wrapper per-step opt-in and `response.default_wrapper`
  config.
- **13** — `default_dsl_in_case_of_exception` fallback DSL +
  `finalResponse` status codes.
- **14** — `guards.mode: stack | closest_only` knob.
- **15** — Every accepted-but-inert config field WARNs at boot.
- **16** — Both scripting backends bind context variables via
  `globalThis["<key>"]`.
- **17** — `.optional.` null suppression in script evaluation.

### Behavioural changes on top of the audit sweep (2026-08-05)

- **`response.default_wrapper` default flipped `false` → `true`** —
  Java parity. Every ReturnStep without an explicit `wrapper:` now
  wraps its value in `{"response": <value>}`. Per-step `wrapper: false`
  still opts out; `response.default_wrapper: false` in config restores
  the raw-body default. Sweep: 26 test assertions in 9 files + 33
  `.test.yml` scenarios updated to match.
- **WebSocket layout renamed to `WS/{inbound,outbound}/`** —
  canonical shape. Inbound frame DSLs live under
  `DSL/<project>/WS/inbound/<path>.yml`; outbound feed configs under
  `DSL/<project>/WS/outbound/<name>.yml`. Legacy layouts
  (`DSL/<project>/WS/*.yml` and `DSL/<project>/sources/*.yml`) still
  work with a boot-time WARN pointing at the new location. URLs
  unchanged.

### Added

- **Source-of-truth parse gate.** `compat/java-ruuter/` mirrors 42
  Java Ruuter DSL files (pinned at `github.com/buerokratt/Ruuter@0454d08c`)
  with MIT attribution preserved in `compat/README.md`. New CI step
  in `tests.yml` on both Boa and QuickJS jobs runs `dsl-lint`
  against the corpus; parse errors fail the build. Three warnings
  are expected — all on Java demos of intentionally unreachable
  steps; see `compat/EXPECTED-BASELINE.md`.

### Book

- New **"Configuration deep dive"** section (`book/src/config/`) —
  10 tutorial pages covering the post-audit config surface:
  `response-wrapper`, `guards-mode`, `default-exception-dsl`,
  `internal-requests`, `proxy-trust`, `listeners`, `unix-sockets`,
  `scripting-limits`, `inert-fields`, plus an overview.
- `book/src/ops/configuration.md` — removed the dead `idempotency:`
  block (feature was removed in v0.7.0; setting it now is inert);
  added every post-audit config knob missing from the file.
- `book/src/reference/reserved-subdirs.md` — new `WS/inbound/` +
  `WS/outbound/` layout; legacy paths marked deprecated with WARN.
- `book/src/ws/{server,sources}.md` — canonical layout updated.

### Notes for partners pulling this pre-release

- The 0.8.0-rc.1 (2026-07-27) publish infrastructure applies
  verbatim — multi-arch, cosign, SBOM, Trivy, smoke test.
- `main` is still reserved for the future `v1.0.0` stable release.
  This RC is cut from `dev`.

## [0.8.0-rc.1] - 2026-07-27

First **pre-release** cut for partner testing. Not GA. Publishes as
`turnerrainer/ruuter:0.8.0-rc.1` on Docker Hub and
`ghcr.io/turnerrainer/ruuter:0.8.0-rc.1` on GHCR. Pre-release tags do
NOT move `:latest` or `:major.minor` — casual pullers on `:latest`
are unaffected until a stable release ships.

### Added

- **Multi-arch container publish workflow** — `.github/workflows/publish.yml`.
  Builds `linux/amd64` + `linux/arm64` via `docker/setup-qemu-action` +
  `docker/setup-buildx-action`. Publishes to Docker Hub and GHCR.
  Supports stable (`vX.Y.Z`) and pre-release (`vX.Y.Z-suffix`) tag
  shapes; pre-releases publish only the specific version tag.
- **Cosign keyless image signing** (Sigstore OIDC), **SPDX SBOM**, and
  **in-toto provenance** attached to every multi-arch manifest. Verify
  recipe in `book/src/ops/docker.md`.
- **Trivy vulnerability scan** in the publish workflow, gated on
  HIGH/CRITICAL fixed CVEs. Blocks signing.
- **Smoke test in publish workflow** — every per-arch image is booted
  under QEMU on the runner and probed for `/health` + `/samples/ping`
  before cosign runs. A signed image is a working image.
- **Reproducible image layer timestamps** via `SOURCE_DATE_EPOCH` +
  `outputs: type=image,rewrite-timestamp=true`.
- **Native arm64 in the test matrix** — `ubuntu-24.04-arm` runners
  added to `tests.yml` for both `boa` and `quickjs` feature sets.
- **`cargo-deny` in the security workflow** alongside `cargo-audit`.
  Config: `deny.toml`. License allow-list (Apache-2.0-compatible
  only, no GPL/AGPL/SSPL), ban on wildcards, refuse git-URL deps.
  Advisory exceptions mirrored between `deny.toml` and
  `.cargo/audit.toml`.
- **`SECURITY.md`** — private disclosure recipe, response SLA,
  supply-chain posture inventory.
- **DSL hot-reload** (opt-in via `dsl.allow_dsl_reloading`, default
  `false`). `notify`-backed filesystem watcher + `ArcSwap` atomic
  publish; HTTP DSL tree, guards, and OpenAPI cache reload without
  a server restart. Source configs, trigger DSLs, `constants.ini`
  and `ruuter.yaml` explicitly do **not** reload. Dev-only —
  combined with a writable DSL mount it is RCE via `${JS}`.
- **`#{KEY}` alternate constant-interpolation syntax** (task 067) —
  visually pairs with `${runtime}`. Both syntaxes tokenise
  identically and produce the same substituted DSL. `[#KEY]` retained
  for backward compat with a soft-deprecation stance (may be
  deprecated in a future major release; new DSLs should prefer
  `#{KEY}`).
- **`DSL/samples/GET/constants/demo.yml`** and matching test —
  runnable proof that both constant syntaxes resolve at parse time.
- **First-time-user "Getting started" chapters** in the book:
  Prerequisites → Run it locally → Watch the automated tests pass
  → Try the Postman collection → What to read next.
- **Postman assets** committed under `postman/` — collection +
  environment + regeneration recipe.
- **Book-wide runnable examples** — every DSL step page and every
  applicable framework/dsl page now carries at least one
  copy-clean `curl` request block + labelled response block, with
  responses captured against a live server.
- **Light-on-white book theme** modelled on Apache Arrow docs.

### Changed

- **`Cargo.lock` is now tracked** (was gitignored). The Dockerfile
  `COPY`s it; a fresh CI clone would have failed the build.
- **Every DSL sample in the book converted to pure block-style YAML**
  — no flow-style `{ … }` maps or inline `[ … ]` arrays. Copy-paste
  any snippet straight into a `.yml` file.
- **Book curl examples split** into separate `bash` request blocks
  and labelled response blocks, so the copy button on the command
  block yields a runnable shell line (no `$` prompt to strip, no
  response body to remove).
- **`/health` doc** refreshed to the v0.7.0 slim shape
  (`{"status":"ok"}` — no framework name, no version).

### Notes for partners pulling this pre-release

- The image bakes in `DSL/samples/` so `/samples/*` endpoints work
  out of the box. Mount your own `DSL/` tree to override.
- Every published digest is cosign-signed. Verify with the recipe in
  `book/src/ops/docker.md#verify-the-image-cosign`.
- `main` is reserved for the future `v1.0.0` stable release. This
  RC is cut from `dev`.

## [0.7.0] - 2026-07-24

Security-hardening release. Closes 15 findings from the h2ck.me
pre-publication audit (S1–S8, N1–N4, F1, F2) across three review
rounds; adds a `cargo audit` CI gate. Every fix has a regression
test in `tests/security*.rs` (68 tests total, all green).
`cargo audit --deny warnings` is clean.

Also in this batch (release audit sweep, 2026-07-24):

- `state.delete` accepts `remove:` as a serde alias so DSL authors
  from a Java Ruuter or Redis background can reach for either verb.
  Verified end-to-end via the loader parse path
  (`src/steps/state.rs` tests).
- `dsl-test`'s `mock-http` and `trigger-inject` modes now build the
  harness with `internal_requests.block_private_networks=false` so
  DSLs under test can reach the in-process mock upstream on
  127.0.0.1. Production behaviour of `check_ssrf` is unchanged;
  only the test-runner process opts out of the private-network gate.
- Repo-wide `cargo fmt` applied and `[lints.clippy]` posture in
  `Cargo.toml` promoted `-D warnings` to a hard CI gate with a
  small, documented allowlist for test-fixture patterns.
- Book: five `v1.0.0` references corrected to `v0.7.0` in
  framework/tracing, framework/self-call-optimization,
  framework/pipeline, reference/non-goals, reference/changelog.
- `DSL/samples/POST/idempotent-transfer.yml` header rewritten to
  describe the DSL-authored idempotency pattern (framework-level
  handling was removed in this same release).

### Breaking

- **Framework-level `Idempotency-Key` handling removed.** The
  framework no longer caches or replays responses by
  `Idempotency-Key`; the `Idempotency-Replayed` response header is
  never emitted. Two identical POSTs with the same key both execute
  the DSL. DSL authors implement idempotency via `state.get` /
  `state.set` with their own identity + body-hash keys — see
  `book/src/dsl/idempotency-pattern.md`. This gives consumers control
  over what "same request" means (body canonicalisation, caller
  identity, tenant scope) instead of the framework guessing.
  Closes h2ck.me findings **S1** (missing body-hash in dedup key —
  cross-caller replay) and **S5** (`Idempotency-Replayed: true`
  oracle for probing keys). Config field
  `internal_requests.idempotency` and struct `IdempotencyConfig` are
  removed; existing config files with that block must drop it.

### Security

- **S2 — SSRF allowlist exact origin match.** `check_ssrf` previously
  used `starts_with` against `internal_requests.allowed_urls`, so an
  operator writing `http://api.example.com` (no trailing slash)
  would accept a lookalike `http://api.example.com.evil.tld/x`. The
  check now parses both the entry and the request URL, requires an
  exact `scheme://host:port` match, and only prefix-matches the path
  portion when the entry itself has a path. Bare-origin entries
  still work — they just no longer admit substring lookalikes.
- **S3 — `internal_requests.disabled` honoured by every transport.**
  The disabled guard now runs at the very top of
  `HttpClient::request`, before the task-044 self-call short-circuit,
  the `unix://` scheme handler, and the `unix_socket_map` alias
  dispatch. When outbound is disabled, no transport slips past.
- **S4 — `X-Forwarded-For` trusted-proxy gating.** New
  `proxy.trusted: [ip, ...]` config. Only when the direct TCP peer's
  IP is in that list does the framework promote `X-Forwarded-For`
  (or `X-Real-IP`) into `incoming.origin`. Otherwise `origin`
  reflects the socket peer, so a direct caller can't spoof the value
  downstream code keys off (audit logs, rate-limit keys, self-call
  bookkeeping). The raw header is still visible in
  `incoming.headers`. Empty `proxy.trusted` (default) is the safe
  posture for direct-exposed deployments. Also newly exposed:
  `incoming.origin` as a first-class field in the DSL scripting
  scope (both Boa and QuickJS backends).
- **S6 — outbound redirects no longer followed transparently.** The
  reqwest client is now built with `redirect(Policy::none())` so a
  whitelisted upstream can't 302 the call to a blocked target
  (`169.254.169.254` and friends). DSLs that legitimately need to
  chase a `Location` header must issue a second `http.<verb>` step —
  which re-runs the SSRF check on the new target.
- **S7 — `/health` no longer leaks framework name + version.** The
  handler now returns `{"status":"ok"}`. Downstream advisory-matching
  against Ruuter builds is no longer possible without an
  operator-shipped admin surface.
- **S8 — vulnerable / unmaintained dependencies dropped.** Replaced
  `serde_yml 0.0.12` (RUSTSEC-2025-0068 unsound, unmaintained) with
  the community fork `serde_yaml_ng 0.10`. Bumped `boa_engine`
  0.19 → 0.20, which drops `fast-float 0.2.0` (RUSTSEC-2025-0003
  SIGSEGV) in favour of `fast-float2` and also drops the
  `libyml 0.0.5` transitive (RUSTSEC-2025-0067). `anyhow` bumped
  to 1.0.104 to close RUSTSEC-2026-0190. `cargo audit` now reports
  0 vulnerabilities; only unmaintained-transitive warnings for
  `instant` and `paste` remain.
- **N1 — path-scoped SSRF allowlist entries enforce segment
  boundary.** After the S2 fix, path-scoped entries such as
  `http://api.example.com/v1` were still matched via `starts_with`,
  which admitted `/v1anything`. The check now requires the next
  character after the entry-path to be `/`, `?`, `#`, or
  end-of-string — the same segment-boundary rule browsers apply.
- **N2 — `X-Forwarded-For` leftmost IP only.** When the peer is
  trusted, only the LEFTMOST comma-separated value that parses as
  an `IpAddr` becomes `incoming.origin`. A non-IP leftmost value
  (misconfigured proxy or spoof attempt) is refused and the
  framework falls back to the socket peer. Downstream DSLs that key
  on `origin` no longer see attacker-controlled substrings.
- **N3 — trusted-proxy list canonicalises IPv4-mapped IPv6.** Both
  the peer IP and each `proxy.trusted` entry are parsed as `IpAddr`,
  and IPv4-mapped IPv6 (`::ffff:127.0.0.1`) is folded back to plain
  IPv4 before comparison. An operator writing `trusted: ["127.0.0.1"]`
  keeps working across dual-stack listener quirks.
- **N4 — default outbound blocklist for private / link-local
  ranges.** New `internal_requests.block_private_networks` config,
  defaults to `true`. Outbound TCP to loopback (127/8, ::1),
  link-local (169.254/16, fe80::/10), unspecified, RFC-1918
  (10/8, 172.16/12, 192.168/16), carrier-grade-NAT (100.64/10) or
  ULA (fc00::/7) is rejected before dispatch — closing the
  cloud-metadata SSRF exposure that the empty-allowlist default
  used to permit. Self-call short-circuits and UDS transports are
  unaffected. Operators who legitimately need a private-network
  sidecar over TCP loopback either add it to `allowed_ips` /
  `allowed_urls` or set `block_private_networks: false`.
- **F1 — trailing-slash SSRF allowlist entries admit their subpaths.**
  The N1 boundary check rejected legitimate requests when the operator
  wrote the recommended trailing-slash form (`http://api/v1/`), because
  the check applied to the character AFTER the trailing `/` — which had
  already been consumed by `starts_with`. `allow_entry_matches` now
  short-circuits when the entry itself ends at a URL delimiter (`/`,
  `?`, `#`); the boundary is already closed. Also extended the tail
  delimiter set to include `&` so query-scoped entries
  (`http://api/v1?tok=X`) admit `?tok=X&extra=1`.
- **F2 — `block_private_networks` follows DNS.** Previously the
  blocklist only ran when the URL host parsed as an IP literal, so
  `http://localhost/`, `http://metadata.google.internal/`, and any
  attacker-controlled DNS name bypassed the check entirely.
  `check_ssrf` is now async and resolves hostnames via
  `tokio::net::lookup_host`; a single private / link-local hit in the
  resolved address set rejects the request. Explicit entries in
  `allowed_ips` / `allowed_urls` still opt the hostname back in.

### CI

- **Task 056 — `cargo audit` gate.** New `.github/workflows/security.yml`
  runs `cargo audit --deny warnings` on every push / PR and on a
  weekly cron so a fresh advisory against unchanged `Cargo.lock`
  still fires. Documented exceptions live in `.cargo/audit.toml`
  with a rationale and review date. Currently exempted:
  RUSTSEC-2024-0384 (`instant` unmaintained, transitive) and
  RUSTSEC-2024-0436 (`paste` unmaintained, transitive) — both
  reviewed 2026-10-01.

## [0.6.6] - 2026-07-19

### Added

- **Task 045 — pre-parsed expression registry (redesign #1).** At
  boot, walks the loaded DSL tree (HTTP DSLs + guards + trigger DSLs)
  and extracts every unique `${...}` and `$=...=` expression source
  into an `ExpressionRegistry`. On the QuickJS backend, each session
  lazily compiles-on-first-use per expression (combined define+invoke
  in one eval to avoid double-parse) and marks a `Vec<AtomicBool>`
  slot; subsequent evals of the same expression in the same session
  invoke `__fn_<id>()` — a tiny string that parses in microseconds.
  Boa backend ignores the registry (no durable place to cache
  functions given its `!Send` context).

### Perf (compound of tasks 051 + 036 + 045)

3-run median on developer laptop, `scripting-boa` default vs
`--features scripting-quickjs`:

| Scenario | Boa | QJS+036 | **QJS+036+045** | Δ vs Boa | Δ vs QJS+036 |
|---|---:|---:|---:|---|---|
| guarded | 1,401 rps | 6,118 rps | **6,955 rps** | **+396%** (5×) | +14% |
| js-heavy | 3,245 rps | 7,906 rps | 7,735 rps | +138% (2.4×) | parity |
| path-params | 2,098 rps | 8,486 rps | 8,111 rps | +286% (3.9×) | parity |
| thin-dsl (037 fast-path) | 77,777 rps | 80,027 rps | 80,398 rps | parity | parity |

Where 045 shines: DSLs that evaluate the same expression multiple
times per request (guard chains checking `${incoming.headers.foo}`
from several conditions; `iterate.do` bodies where the same
computation fires per iteration). No regressions on cache-miss-
heavy scenarios (path-params).

### Design notes

- v1 attempt bulk-compiled every registered expression at session
  init. That was slower than QJS+036 alone (per-request sessions
  use 1-3 expressions from a 60+ corpus — bulk cost isn't amortised).
  Reverted; kept the registry, changed to lazy-per-slot compilation
  with `Vec<AtomicBool>` flags. This is the v2 shipped.
- Combined define+invoke `(globalThis.__fn_N = function(){...})()`
  in one eval avoids the earlier design's cache-miss double-parse
  regression.

### Bottom line

Full Boa-perf roadmap now realised on the QuickJS backend:
**2.4-5× throughput vs default Boa** across Boa-hitting DSL
scenarios, with framework baseline and 037's literal fast-path
unchanged. Boa remains the default (no CVE surface); QuickJS is
opt-in for operators who want the compound win.

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

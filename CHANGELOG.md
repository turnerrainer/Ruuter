# Changelog

All notable changes to Ruuter-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

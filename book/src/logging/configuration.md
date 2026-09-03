# Configuration reference

Every knob under `logging:` in `ruuter.yaml`. All optional, sane
defaults. See `src/config/mod.rs::LoggingConfig`.

## Full block

```yaml
logging:
  # === Output shape ===
  format: text           # or `pretty` (ANSI colours) or `json`. Env: RUUTER_LOG_FORMAT.

  # === Per-request access log (INFO) ===
  access_log: true       # one INFO line per completed HTTP request

  # === Per-step INFO trail (Java-parity, issue #37) ===
  log_step_executions: true   # one INFO `Executed` line per step
  log_dsl_runs:        false  # INFO bracket lines around each run (opt-in)

  # === Per-step DEBUG line ===
  step_timing: false     # emit `dsl.step`, `duration_ms` per step

  # === Outbound HTTP body/header dumps (DEBUG) ===
  display_request_content:  false   # log outbound request bodies
  display_response_content: false   # log upstream response bodies
  max_body_bytes:           2048    # cap any body content included

  # === Redaction ===
  redact_headers:
    - authorization
    - proxy-authorization
    - cookie
    - set-cookie
    - x-api-key
    - x-auth-token
  redact_body_fields:
    - password
    - pass
    - secret
    - token
    - access_token
    - refresh_token
    - api_key
    - authorization

  # === Error rendering ===
  print_stack_trace: false     # include error `source()` chain
  meaningful_errors: false     # emit a second WARN line with
                                # the underlying cause message
```

## Per-field reference

### `format`

- **Type**: `text` | `pretty` | `json`
- **Default**: `text`
- **Env override**: `RUUTER_LOG_FORMAT`
- **Effect**: Selects the output format of every log event.
  `text` is compact one-line-per-event terminal-first; `pretty`
  adds ANSI colours + Unicode markers for interactive dev (do
  not pipe to a file); `json` emits one OTel-log-shape object
  per event for production ingest. See
  [Output formats](./formats.md).

### `access_log`

- **Type**: `bool`
- **Default**: `true`
- **Effect**: When `true`, emits one INFO line per completed HTTP
  request with `http.request.method`, `http.route`,
  `http.response.status_code`, `duration_ms`, `dsl.project`,
  `client.address`, `trace_id`. Set to `false` only if a
  downstream (proxy, load balancer) already produces the same
  data — otherwise you lose your primary operational signal.

### `log_step_executions`

- **Type**: `bool`
- **Default**: `true`
- **Effect**: When `true`, emits one INFO `Executed` line per DSL
  step with `dsl.step`, `dsl.step.type`, `duration_ms`,
  `dsl.next.step`, and an `attrs` field carrying step-type-specific
  context. Field names inside `attrs` are deliberately short —
  the step type is already on the line (`(state)`, `(switch)`,
  etc.), so prefixing every field with its type would duplicate.
  Full OTel semantic-convention names still appear on the primary
  access log line and on the OTel span for dashboard portability.
  - **HTTP**: `method`, `url`, `status` (+ `error_route=true` when
    the step is about to branch to its `error:` handler)
  - **switch**: `condition` (0-indexed slot of the matched
    condition in the DSL's `switch:` list) + `expr` (the raw JS
    expression at that slot). When no condition matches, emitted as
    `condition=no_match` (unquoted, snake_case sentinel matching
    the rest of Ruuter's log-attr casing) so a single `condition=`
    filter catches both matched and unmatched runs. The routed
    target appears in the top-level `dsl.next.step` column, so no
    separate `switch.next=…` field is needed.
  - **return**: `status` + optional `wrapper` + `body` (redacted,
    capped preview of the returned value — 80 chars, JSON preview)
  - **state**: `op` + `key`, `hit` (only on `get`), `value`
    (only on `set`, redacted + capped preview)
  - **log**: `msg` (evaluated, capped at 256 bytes)
  - **iterate**: `count` + `as`
  - **template**: `dsl` + `status` + `body` (redacted, capped
    preview of the callee's return value)
  - **assign**: `keys` (sorted, comma-joined)
  - **ws_send**: `mode` + `delivered` (+ `attempted` on fan-out,
    `prefix` on broadcast)
  - **single_flight**: `role` + `key`
  - **http_mock**: `status`

  Body previews (`return.body`, `state.value`, `template.body`) are
  redacted per `redact_body_fields` and capped at 80 bytes — enough
  to identify what happened, small enough to fit on one terminal
  line. Never trace request-body content through these fields;
  they exist to show DSL-produced values.

  Java parity for `LoggingUtils.logStep()` (issue #37). Turn off
  for very high-QPS DSLs where the per-step line is noise;
  `step_timing` (DEBUG) remains a finer-grained alternative.

### `log_dsl_runs`

- **Type**: `bool`
- **Default**: `false` (opt-in)
- **Effect**: When `true`, emits an INFO line at DSL run start
  (`DSL run started` with `dsl.project`, `dsl.total_steps`,
  `dsl.first_step`) and at DSL run end (`DSL run completed` with
  `dsl.steps_ran`, `duration_ms`, `terminated_by` = `return` |
  `end_of_steps` | `iteration_cap` | `error`, plus
  `http.response.status_code` on `return` and `failed_step` on
  `error`). Off by default because the request span already
  brackets each run via `trace_id`, the access log carries the
  wall-clock duration and status code, and the last `Executed`
  step's type usually reveals the terminator — the bracket lines
  triple the log volume for a single-step DSL without adding
  new signal. Turn on when you want an explicit `terminated_by`
  label in the log stream for grep-based triage of long DSLs, or
  when you're bracketing a run without a request span (e.g. an
  event-triggered DSL that doesn't have an HTTP request context).

### `step_timing`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, emits one DEBUG line per DSL step
  with `dsl.step`, `dsl.step.type`, `duration_ms`, `skipped`.
  Needs `RUST_LOG` to also permit DEBUG for the emitting module
  (`RUST_LOG=ruuter_on_rust::steps::engine=debug` or broader).
  A superset of `log_step_executions` on debugging paths — turn
  on for local debugging when you already have INFO lines and
  want the same info at DEBUG level too.

### `display_request_content`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, HTTP steps emit a DEBUG line before
  each outbound call with `http.request.method`, `url.full`,
  `http.request.body`, `http.request.headers`. Body is
  redacted per `redact_body_fields` and capped at
  `max_body_bytes`; headers are redacted per `redact_headers`.
  Java-parity name; same semantics as Java Ruuter's
  `logging.displayRequestContent`.

### `display_response_content`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, HTTP steps emit a DEBUG line after
  each upstream response with `http.request.method`, `url.full`,
  `http.response.status_code`, `http.response.body`,
  `http.response.headers`. Same redaction / cap treatment as
  `display_request_content`.

### `max_body_bytes`

- **Type**: unsigned integer (bytes)
- **Default**: `2048`
- **Effect**: Cap on the serialised length of any body included
  in a log line. Truncation happens after redaction and cuts at
  a UTF-8 char boundary; truncated output ends with `…`. Values
  above ~16384 defeat the point (log-store ingest lines
  routinely rate-limit above that).

### `redact_headers`

- **Type**: list of strings
- **Default**: `["authorization", "proxy-authorization", "cookie", "set-cookie", "x-api-key", "x-auth-token"]`
- **Effect**: Header names whose values are replaced with
  `"[REDACTED]"` in any logged header map. Case-insensitive
  match. Add project-specific auth / session header names —
  do NOT remove the defaults unless the header is genuinely
  not sensitive in your deployment.

### `redact_body_fields`

- **Type**: list of strings
- **Default**: `["password", "pass", "secret", "token", "access_token", "refresh_token", "api_key", "authorization"]`
- **Effect**: JSON body field names whose values are replaced
  with `"[REDACTED]"` in any logged body. Case-insensitive
  match, applied at every nesting depth (including arrays).
  Extend for project-specific PII / secret field names.

### `print_stack_trace`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, on step error emit `cause_chain=…`
  on the ERROR line containing the error's `source()` chain
  formatted as `-> caused by: X -> caused by: Y`, bounded to
  5 hops. Off by default because chains can leak upstream
  schema details. Java-parity name.

### `meaningful_errors`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, on step error emit a second WARN
  line with `cause=<underlying source().to_string()>`. Off
  by default; the primary ERROR is usually enough. Java-parity
  name.

## Defaults, in one line

- Text output; access log and per-step INFO trail on; DSL-run
  bracket lines opt-in; everything else off; sensible redaction
  lists, 2 KiB body cap.

## What the defaults give you

- One INFO access-log line per request with trace_id, method,
  route, status, duration, project, client IP.
- One INFO `Executed` line per step with type + duration + next
  target + per-step-type context in `attrs` (HTTP URL & status,
  switch match, return status, state op/key, log message, etc.).
  This is the Java-parity execution trail (issue #37).
- No DSL-run bracket lines — the request span already frames each
  run via `trace_id`. Turn on `log_dsl_runs` if you want explicit
  `DSL run started` / `DSL run completed` INFO lines around every
  invocation.
- No outbound-body chatter (needs `display_*_content`).
- Errors as one ERROR line, no chain.
- Secrets never appear on any line.

For a 1-step DSL, expect two lines per request in the default
config: one `Executed` and one `http request completed`. Multi-
step DSLs add one line per additional step. For very high-QPS
DSLs where even that is noise, set `logging.log_step_executions:
false` and rely on the access log + OTel spans.

## Interaction rules

- `RUUTER_LOG_FORMAT` env wins over `logging.format`.
- `logging.*` flags don't automatically change `RUST_LOG`. To see
  DEBUG lines from `step_timing`, `display_request_content`, or
  `display_response_content`, `RUST_LOG` must permit DEBUG for
  the emitting module.
- Setting `access_log: false` doesn't disable the request span;
  the OTLP exporter still gets one span per request.

## Cross-links

- [Field vocabulary](./fields.md) — full list of fields each
  flag enables.
- [Redaction](./redaction.md) — how the redaction lists are
  applied.
- [Errors & trace correlation](./errors.md) — how the two error
  flags shape ERROR output.
- [Recipes](./recipes.md) — putting these knobs together for
  common scenarios.

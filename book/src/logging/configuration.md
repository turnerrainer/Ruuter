# Configuration reference

Every knob under `logging:` in `ruuter.yaml`. All optional, sane
defaults. See `src/config/mod.rs::LoggingConfig`.

## Full block

```yaml
logging:
  # === Output shape ===
  format: text           # or `json`. Env: RUUTER_LOG_FORMAT.

  # === Per-request access log (INFO) ===
  access_log: true       # one INFO line per completed HTTP request

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

- **Type**: `text` | `json`
- **Default**: `text`
- **Env override**: `RUUTER_LOG_FORMAT`
- **Effect**: Selects the output format of every log event. See
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

### `step_timing`

- **Type**: `bool`
- **Default**: `false`
- **Effect**: When `true`, emits one DEBUG line per DSL step
  with `dsl.step`, `dsl.step.type`, `duration_ms`, `skipped`.
  Needs `RUST_LOG` to also permit DEBUG for the emitting module
  (`RUST_LOG=ruuter_on_rust::steps::engine=debug` or broader).
  Chatty on high-QPS DSLs — turn on for local debugging, not
  production defaults.

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

- Text output, access log on, everything else off, sensible
  redaction lists, 2 KiB body cap.

## What the defaults give you

- One INFO access-log line per request with trace_id, method,
  route, status, duration, project, client IP.
- Zero per-step chatter.
- No outbound-body chatter.
- Errors as one ERROR line, no chain.
- Secrets never appear on any line.

Which is: the smallest set that answers "what's happening?"
without answering questions nobody asked.

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

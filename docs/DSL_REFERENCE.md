# Ruuter-on-Rust DSL & runtime reference

Reference for LLMs authoring DSLs against Ruuter-on-Rust 0.4.0. Samples first, prose last.

---

## 0. Anatomy

- Route file: `DSL/<project>/<METHOD>/<path>.yml` → `<METHOD> /<project>/<path>`.
- YAML top-level keys = ordered steps. First key = entry step.
- Step control flow: implicit fall-through (source order) OR explicit `next: <step-name>`. Terminate with `next: end`.
- `${...}` = JS expression, single-expression preserves native type; mixed with literal text stringifies.
- `$= ... =$` = whole-line JS form (alternative to `${...}`).
- `[#KEY]` = constants.ini substitution at DSL parse time (before JS eval).

## 1. Steps

### 1.1 `return`

```yaml
respond:
  return: { ok: true, echo: "${incoming.body.value}" }
  status: 202                     # optional; default 200; may be ${...}
  headers:                        # optional
    X-Custom: "yes"
  next: end
```

### 1.2 `assign`

```yaml
compute:
  assign:
    total:   "${items.reduce((a,b)=>a+b.price,0)}"
    now_iso: "${new Date().toISOString()}"
  next: reply
```

### 1.3 `switch`

```yaml
route:
  switch:
    - condition: "${incoming.body.action === 'buy'}"
      next: buy_step
    - condition: "${incoming.body.action === 'sell'}"
      next: sell_step
  next: default_step              # fallthrough if no condition matched
```

### 1.4 `log`

```yaml
audit:
  log: "user=${incoming.headers['x-user']} did=${incoming.body.action}"
  next: reply
```

### 1.5 `state`

Project-scoped in-process store. Ephemeral (see task 029/017).

```yaml
read:  { state: { get: { key: "counter", into: n } }, next: bump }
bump:  { assign: { n2: "${(n ?? 0) + 1}" }, next: write }
write: { state: { set: { key: "counter", value: "${n2}" } }, next: reply }
wipe:  { state: { delete: { key: "counter" } }, next: end }
```

### 1.6 `iterate`

```yaml
work:
  iterate:
    over: "${orders}"             # expression → array
    as: order                     # per-item binding
    max_items: 100                # default 10_000
    do:
      - assign: { net: "${order.qty * order.price}" }
    collect: "${{ id: order.id, net: net }}"   # optional; per-item value
    into: totals                              # collected array bound here
  next: reply
```

- `return` inside `do:` short-circuits the outer DSL.
- Non-array `over` → step errors.

### 1.7 `http`

```yaml
fetch:
  call: http.get                  # http.{get,post,put,patch,delete}
  args:
    url: "https://[#API_HOST]/v1/orders/${id}"
    headers: { Authorization: "Bearer [#API_TOKEN]" }
    query:   { limit: 50 }
    body:    { note: "hi" }       # POST/PUT/PATCH; JSON-serialized
  result: upstream                # binds .response.{status,body,headers}
  timeout: 3000                   # ms; overrides http_request_timeout
  next: reply
```

- Outbound `traceparent` header is auto-forwarded from the request context (skip by setting it explicitly in `headers`).
- SSRF allow-list (`internal_requests.{disabled,allowed_urls,allowed_ips}`) enforced before send.
- Response size capped at `http_response_size_limit`; over-cap = step error.
- Upstream status filtered against `http_codes_allow_list` when non-empty; disallowed = step error.

### 1.8 `template`

Recursive DSL invocation. Calls another DSL in the same project through the shared engine.

```yaml
fetch:
  template: templates/user-profile   # project-relative path (no ext)
  request_type: GET                  # default GET
  body:    { name: "alice" }         # overrides for callee's incoming.body
  query:   { verbose: "1" }
  headers: { X-Trace: "yes" }
  result: profile                    # binds .response.{status,body,headers}
  next: reply
```

Target must exist at `DSL/<project>/<METHOD>/<template>.yml`. Missing → step error.

### 1.9 `ws_send`

```yaml
reply:                              # inside a WS DSL — sends to caller
  ws_send:
    payload: { type: "echo", got: "${incoming.body}" }

fanout:                             # broadcast to every matching conn
  ws_send:
    broadcast_prefix: "client:"     # sends to every id starting with "client:"
    payload: { note: "server closing in 5 min" }

direct:                             # target specific connection id(s)
  ws_send:
    to: "${target_cid}"             # string OR array of strings
    payload: { dm: "${incoming.body.text}" }
```

Priority: `broadcast_prefix` > `to` > `context.connection_id` (implicit). HTTP-context DSLs without any of these = step error.

### 1.10 `declaration`

DSL metadata step. No runtime effect except:
- `override_ancestors: true` on a guard DSL bypasses ancestor guards (task 020).
- OpenAPI generator uses `description`, `allowed_body`, `allowed_header`, `allowed_params`.

```yaml
declaration:
  description: "Cancel an order and archive its audit trail."
  allowed_body:   [order_id, reason]
  allowed_header: [Authorization, Idempotency-Key]
  override_ancestors: false          # guard-only field

cancel:
  # ... steps ...
```

## 2. Context variables (inside `${...}`)

| Binding | Populated when |
|---|---|
| `incoming.body`   | Request body when Content-Type is JSON; else `{}`; WS frame parsed JSON (or `{value: "…"}` for non-JSON text) |
| `incoming.query`  | URL query params (all strings) |
| `incoming.params` | Alias of query. `incoming.params.pathParams` = array of trailing URL segments after DSL match |
| `incoming.headers`| Lower-cased header names → string values |
| `incoming.connection_id` | WS-only. Per-client id like `"client:<32-hex>"` |
| `<varname>`       | Anything bound via `assign`, `state.get into:`, `http.result`, `template.result`, `iterate.into`, guard-side `assign` |

## 3. Path parameters (task 018)

One DSL serves any URL suffix. Stripped trailing segments arrive as `incoming.params.pathParams` (array, URL order).

```
GET /svc/things              → pathParams=[]
GET /svc/things/abc          → pathParams=["abc"]
GET /svc/things/abc/legs     → pathParams=["abc","legs"]
```

More-specific DSL wins over path-param fallback. See `DSL/samples/GET/things.yml`.

## 4. Guards

`<stem>.guard.yml` (sibling convention) OR `.guard.yml` / `.guard` (in-folder convention, Java parity) → applies to every DSL under that folder.

```yaml
# DSL/svc/POST/api.guard.yml — sibling convention
check:
  switch:
    - condition: "${!incoming.headers['authorization']}"
      next: deny
  next: allow
allow: { return: { auth: ok }, next: end }
deny:  { status: 401, return: { error: "no token" }, next: end }
```

```yaml
# DSL/svc/POST/vault/.guard.yml — in-folder convention (task 019)
# Protects everything under vault/. Same semantics as sibling.
```

Rules:
- Guard returning status ≥ 400 short-circuits, its response body/status becomes the response.
- Guard can `assign` variables the main DSL then reads (auth subject, parsed token, etc.).
- Multiple ancestor guards stack outermost-first; ALL must pass unless one has an override.
- **Override** (task 020): `declaration.override_ancestors: true` on a guard makes it REPLACE all ancestor guards for its subtree. Longest-key override wins if multiple match.

## 5. Constants

`constants.ini` at Ruuter's cwd; mounted read-only in Docker.

```ini
# Section headers accepted for Java-Ruuter compatibility but do not scope keys.
[DSL]
API_HOST=api.example.com
API_TOKEN=abc123
```

- Referenced as `[#KEY]` inside DSL YAML.
- Substituted at parse time (before JS eval), so `[#KEY]` values appear as literal text in the parsed DSL.
- Missing key in DSL body = literal `[#KEY]` at runtime.
- Missing key in WS source config = load-time error (`resolve_constants`).
- Secrets management is out of scope: mount the resolved file, rotate via the pipeline.

## 6. WebSocket server

`DSL/<project>/WS/<path>.yml` → clients connect at `ws://host/<project>/<path>`.

```yaml
# DSL/svc/WS/chat.yml
on_frame:
  switch:
    - condition: "${incoming.body.type === 'join'}"
      next: welcome
  next: broadcast

welcome:
  ws_send:
    payload: { type: "welcome", you: "${incoming.connection_id}" }
  next: end

broadcast:
  ws_send:
    broadcast_prefix: "client:"
    payload: { from: "${incoming.connection_id}", msg: "${incoming.body.msg}" }
  next: end
```

DSL runs once per inbound text/binary frame. Handshake headers + query snapshotted at upgrade (identical every frame). Non-JSON text frames arrive as `{value: "<text>"}`.

### 6.1 Connection tags — `ws_tag` + `broadcast_where`

Every connection carries a string→string tag map. A WS server DSL stamps tags on the **originating** connection with `ws_tag`, then any later `ws_send` can fan out to exactly the connections whose tags match — without maintaining an external "which socket belongs to whom" directory.

```yaml
# On connect: authenticate the handshake cookie, then record who this socket is.
stamp_identity:
  ws_tag:
    set:
      user:  "${u.personal_code}"
      roles: "${',' + u.roles.join(',') + ','}"   # delimited → token-exact `contains`
  next: end
```

```yaml
# Elsewhere (any DSL sharing the process, e.g. an internal HTTP route):
notify_admins:
  ws_send:
    broadcast_where:
      tag: "roles"
      contains: ",admin,"        # or: equals: "16 chars exactly"
    payload: { type: "notice" }
  next: end
```

- `tag`, and the `equals` / `contains` operand, are evaluated through the script engine — `${…}` works.
- Exactly one of `equals` (whole-value match) / `contains` (substring) is required. Both `tag` and the operand must resolve to a non-empty string — an empty `contains` would match every tagged connection and is almost always an unresolved `${…}`, so it's rejected outright.
- A connection missing the tag never matches.
- `ws_tag` errors outside a WS DSL (no `connection_id`). Tags are process-local and dropped on disconnect.
- Addressing priority in `ws_send`: `broadcast_where` › `broadcast_prefix` › `to` › originating connection.

## 7. WebSocket sources (upstream feeds)

`DSL/<project>/sources/<name>.yml` → outbound client to an upstream WS. Each frame dispatches to `DSL/<project>/triggers/<channel>/<key>.yml` (with `_default.yml` fallback).

```yaml
# DSL/svc/sources/stock-feed.yml
kind: websocket
url: "wss://stream.example.com/v2"
headers:
  X-API-Key: "[#feed_key]"       # sent on upgrade (task 021)
on_connect:
  - send_json: { action: auth, key: "[#feed_key]" }
  - send_json: { action: subscribe, bars: ["AAPL", "MSFT"] }
dispatch:
  channel: "$.T"                 # dot-path in the inbound JSON → channel
  key:     "$.S"                 # dot-path → key
reconnect:
  initial_backoff_ms: 500
  max_backoff_ms: 60000
  jitter: true
```

```yaml
# DSL/svc/triggers/bars/AAPL.yml — per-symbol handler
handle:
  state: { set: { key: "last.AAPL", value: "${incoming.body.c}" } }
  next: end
```

```yaml
# DSL/svc/triggers/bars/_default.yml — catches every other symbol
handle:
  state: { set: { key: "last.${incoming.body.S}", value: "${incoming.body.c}" } }
  next: end
```

Sources run under a supervisor: crash → exponential-backoff restart. Source's own sink is registered as `source:<project>:<name>` so a trigger DSL can `ws_send: { to: "source:svc:stock-feed", payload: ... }` back upstream (e.g. mid-stream subscription changes).

## 8. Framework endpoints (built-in, not from DSL)

| Method + path | Purpose |
|---|---|
| `GET /health` | `{"status":"ok","service":"ruuter-on-rust","version":"…"}` |
| `GET /_/openapi.json` | OpenAPI 3.1 spec auto-generated from every DSL (task 027/035) |
| `GET /_/sources` | Source supervisor health. Off by default; enable with `RUUTER_ADMIN_ENABLED=true` |

## 9. Response headers (framework adds)

| Header | On |
|---|---|
| `traceparent`         | Every response. Echoes the adopted or generated W3C traceparent |
| `X-Trace-Id`          | Every response. 32-hex trace id extracted from traceparent |
| `Idempotency-Key`     | Every response involving an active Idempotency-Key |
| `Idempotency-Replayed`| `true` when the response came from the cache instead of running the DSL |
| `Access-Control-*`    | When `cors.allowed_origins` is non-empty and Origin matches |
| Any `response_default_headers` from config | Every response, unless the DSL explicitly set the same header |

## 10. Framework request handling (before DSL)

1. Method allow-list (`incoming_requests.allowed_method_types`) → 405 if not allowed.
2. CSRF Origin check (`csrf.allowed_origins` + `csrf.enforce_on_methods`) → 403 if empty/mismatched (bypassed when `allowed_origins` is empty).
3. If-Match presence (`optimistic_concurrency.require_if_match` + `enforce_on_methods`) → 428 if missing (opt-in).
4. JSON body parse (Content-Type application/json only) → 400 on malformed.
5. Idempotency-Key lookup (if method in `idempotency.methods`) → cache hit replays response.
6. Guard chain (outermost-first; override guards replace stack) → guard's status ≥ 400 short-circuits.
7. Main DSL execution.
8. Response written; framework injects headers listed in §9.

## 11. Configuration

Resolution priority:
1. `--config <path>` CLI flag
2. `RUUTER_CONFIG=<path>` env var
3. `./ruuter.yaml` or `./ruuter.yml`
4. Built-in defaults

Full annotated example: `DSL/samples/ruuter.yaml.example`. Every field has a safe default; supply only overrides.

Key knobs (with default):

```yaml
port: 8080
http_request_timeout: 15000            # ms
max_step_recursions: 10000             # engine step-transition cap
http_response_size_limit: 16777216     # bytes
http_codes_allow_list: []              # empty = accept all upstream statuses
response_default_headers: {}

cors:              { allowed_origins: [], allow_credentials: false }
incoming_requests: { allowed_method_types: [GET, POST, PUT, PATCH, DELETE, OPTIONS] }
internal_requests: { disabled: false, allowed_urls: [], allowed_ips: [] }
csrf:              { allowed_origins: [], enforce_on_methods: [POST, PUT, PATCH, DELETE] }
idempotency:       { enabled: true, ttl_seconds: 86400, methods: [POST, PUT, PATCH, DELETE] }
optimistic_concurrency: { require_if_match: false, enforce_on_methods: [PUT, PATCH, DELETE] }
scripting:         { max_loop_iterations: 1000000, max_stack_size: 400 }
```

## 12. Env vars

| Var | Effect |
|---|---|
| `RUST_LOG`                     | Log filter (default `info`) |
| `RUUTER_CONFIG`                | Config file path (see §11) |
| `RUUTER_ADMIN_ENABLED=true`    | Expose `GET /_/sources` |
| `OTEL_EXPORTER_OTLP_ENDPOINT`  | Enable OTLP span export |
| `OTEL_SERVICE_NAME`            | OTel service name (default `ruuter-on-rust`) |

## 13. Reserved subdirectories under `DSL/<project>/`

| Directory | Purpose |
|---|---|
| `triggers/`         | Event-trigger DSLs dispatched by sources. NOT routed as HTTP |
| `sources/`          | WS source configs (§7). NOT routed |
| `cronmanager-jobs/` | CronManager job configs. NOT routed |
| `WS/`               | WebSocket server DSLs (§6) |
| `GET/POST/PUT/PATCH/DELETE/OPTIONS/` | HTTP method routes |

## 14. Failure modes at a glance

| Symptom | Cause |
|---|---|
| 404 with `{"error":"Not Found"}`    | No DSL matches even after path-param stripping |
| 405 with `{"error":"Method Not Allowed"}` | Method not in `incoming_requests.allowed_method_types` |
| 400 with `{"error":"invalid JSON body: …"}` | Malformed JSON when Content-Type is `application/json` |
| 403 with `{"error":"CSRF: origin not allowed"}` | Origin/Referer not in `csrf.allowed_origins` |
| 428 with `{"error":"If-Match header is required for this method"}` | `optimistic_concurrency.require_if_match=true` and header missing |
| 500 with `{"error":"outbound HTTP is disabled …"}` | Step tried an outbound call and `internal_requests.disabled=true` |
| 500 with `{"error":"url not in internal_requests.allowed_urls: …"}` | SSRF allow-list rejection |
| 500 with `{"error":"upstream response body … exceeds http_response_size_limit …"}` | Upstream body over cap |
| 500 with `{"error":"upstream status … not in http_codes_allow_list"}` | Upstream status filtered |
| 500 with `{"error":"template not found: … (project=…)"}` | Template step target missing |
| 500 with `{"error":"ws_send: no such connection '…'"}` | `to:` id not registered in WsRegistry |
| 500 with `{"error":"outbound HTTP is disabled …"}` | Framework SSRF stance |

## 15. Ephemeral state caveat

`state` step uses an in-process store. Not durable, not shared across replicas. See task 017 (blocked on architectural decision). For persistent state, front with Resql — Ruuter is a stateless gateway by design.

## 16. JS gotchas inside `${...}`

- **Object literal at expression start** is parsed as a block, not an object. Wrap in parens:
  ```yaml
  ok:   "${({ id: order.id, ok: true })}"
  wrong: "${{ id: order.id, ok: true }}"   # SyntaxError: expected ';'
  ```
- **Optional chaining** works: `${incoming.body?.user?.name ?? 'anon'}`.
- **`Date.now()` / `new Date()`** available; JSON-serialize with `.toISOString()` for stable string output.
- **No `console.log`** — use the `log` step instead.
- **Boa runtime limits** (see §11): infinite loop / deep recursion aborts the eval with a `Script evaluation error`.

## 17. What Ruuter deliberately does NOT do

- IAM / JWT verification (task 028 — DSL author's job via a guard).
- ETag value validation (task 030 — DSL validates against Resql state).
- Multi-replica Idempotency-Key sharing (task 029).
- Secrets fetching from Vault/KMS/etc. (§5 — mount the resolved file).
- Scheduled jobs — that's CronManager firing HTTP at Ruuter routes.

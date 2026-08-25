# declaration

DSL metadata: OpenAPI hints, request-field allowlists, response
schemas, and per-DSL posture flags. Every field is optional; a DSL
without any declaration still loads and runs (Ruuter emits a WARN
at boot per un-declared HTTP DSL — see [Configuration](../../ops/configuration.md)).

```yaml
declaration:
  description: "Create a user record."
  version: "1"
  namespace: "identity"

  # === Structured allowlist (task 070, preferred) ===
  allowlist:
    body:
      - field: userName
        type: string
        required: true
        format: email
        description: "Login handle."
      - field: age
        type: integer
        default: 18
      - field: tags
        type: array
        items:
          field: __item__
          type: string
    params:
      - field: correlation_id
        type: string
        format: uuid
    headers:
      - field: Authorization

  # === Typed response schema (task 070) ===
  returns:
    - field: id
      type: integer
      required: true
    - field: email
      type: string
      format: email

  # === Per-DSL posture ===
  strict: true                    # reject unknown body/query/header keys with 400
  override_ancestors: false       # only meaningful on guard DSLs

  # === Legacy flat allowlist (still supported) ===
  # allowed_body: [userName, age]
  # allowed_params: [correlation_id]
  # allowed_header: [Authorization]
```

## What each field does

### `description`, `version`, `namespace`

Metadata. Surfaced in the auto-generated OpenAPI spec. Runtime
no-op.

### `allowlist.body` / `allowlist.params` / `allowlist.headers`

The **structured** form. Each entry is a `DslField` with per-field
metadata:

- `field` — the field name (required).
- `type` — OpenAPI type: `string` (default), `integer`, `number`,
  `boolean`, `array`, `object`.
- `required` — `true` puts the field in the OpenAPI `required` array;
  in the request body allowlist for POST/PUT/PATCH, missing required
  fields cause a `Field missing: X` error (500). Default `false`.
- `format` — OpenAPI format hint (`email`, `uuid`, `date-time`, …).
- `description` — human-readable, shows in the spec.
- `default` — default value; emitted as `default:` in the spec.
- `items` — for `type: array`, the item schema (recursive `DslField`).

### `allowed_body` / `allowed_params` / `allowed_header`

The **legacy flat** form: plain string lists. Kept for back-compat
with older DSLs and Java-shape corpora. When both are set, the
legacy form wins over the structured form (Java-parity precedence).

Prefer the structured form for new DSLs — it produces a richer
OpenAPI spec.

### `returns` (task 070)

Structured response schema. Each entry is a `DslField` (same shape
as body allowlists). Used by the OpenAPI generator to emit typed
2xx response schemas instead of the fallback `{"type": "object",
"additionalProperties": true}`.

```yaml
declaration:
  returns:
    - field: id
      type: integer
      required: true
    - field: email
      type: string
      format: email
      required: false
```

### `strict` (task 070)

Per-DSL opt-in to reject unknown request keys with **400 Bad Request**
instead of silently filtering them. Only meaningful when at least one
allowlist is declared (with no allowlist, "unknown" isn't defined).

```yaml
declaration:
  strict: true
  allowlist:
    body:
      - field: userName
```

A request that carries `{"userName": "alice", "surprise": "extra"}`
would previously succeed (Ruuter silently dropped `surprise`). With
`strict: true` it now returns:

```json
{"error": "Unexpected field in body: surprise"}
```

`traceparent` on request headers is always allowed under `strict`
even if it isn't in the header allowlist (framework-injected).

### `override_ancestors`

Only meaningful on guard DSLs. `true` = this guard REPLACES ancestor
guards for its subtree; `false` (default) = guards stack. See
[Guards](../guards.md).

## Effects at request time

1. **Allowlist filtering.** Body / query / header maps are restricted
   to declared field names before the DSL sees them.
2. **Required-field check.** On POST, every declared body field must
   be present. On GET, every declared body field must be present in
   the query string (Java-parity). Missing → 500.
3. **Strict-key rejection.** When `strict: true`, any request key
   not in the effective allowlist → 400.
4. **OpenAPI generation.** Full spec produced from declaration
   metadata; consumers generate typed clients.
5. **Missing-declaration WARN.** Boot-time WARN per HTTP DSL without
   a declaration (gated by `dsl.warn_on_missing_declaration`, default
   on). Silence via config.

## Missing declaration — what happens

Nothing at request time. The DSL loads and runs unchanged (Java-
parity permissive posture). The only signals:

- One WARN line at boot per undeclared HTTP DSL.
- The auto-generated OpenAPI spec for that route has minimal
  metadata — just a synthesised description, no typed body / params
  / responses.

Silence the WARN with:

```yaml
# ruuter.yaml
dsl:
  warn_on_missing_declaration: false
```

## Runnable example — typed spec generated from a declaration

DSL:

```yaml
# DSL/samples/POST/typed-users/create.yml
declaration:
  description: "Create a user."
  allowlist:
    body:
      - field: userName
        type: string
        required: true
        format: email
      - field: age
        type: integer
        required: false
        default: 18
  returns:
    - field: id
      type: integer
      required: true

reply:
  status: 201
  return:
    id: 42
```

Query:

```bash
curl -s http://localhost:8080/_/openapi.json | \
     jq '.paths["/samples/typed-users/create"].post | {requestBody, responses}'
```

Output (elided for brevity):

```json
{
  "requestBody": {
    "required": true,
    "content": {
      "application/json": {
        "schema": {
          "type": "object",
          "properties": {
            "userName": {"type": "string", "format": "email"},
            "age": {"type": "integer", "default": 18}
          },
          "required": ["userName"]
        }
      }
    }
  },
  "responses": {
    "201": {
      "content": {
        "application/json": {
          "schema": {
            "type": "object",
            "properties": {
              "id": {"type": "integer"}
            },
            "required": ["id"]
          }
        }
      }
    }
  }
}
```

## Runnable example — strict-key rejection

Same DSL as above, plus `strict: true`:

```yaml
declaration:
  strict: true
  allowlist:
    body:
      - field: userName
        type: string
        required: true
```

Client:

```bash
$ curl -sSD - http://localhost:8080/samples/typed-users/create \
    -X POST -H 'content-type: application/json' \
    -d '{"userName":"alice","surprise":"extra"}'

HTTP/1.1 400 Bad Request
content-type: application/json
...

{"error":"Unexpected field in body: surprise"}
```

Remove `strict:` (or set it to `false`) and the same request
succeeds — `surprise` is silently filtered out of `${incoming.body}`.

## Runnable example — guard override

`DSL/samples/POST/ops/inject-fault.guard.yml` uses
`override_ancestors` to *replace* the folder-wide guard rather than
stacking on it — so `inject-fault` is always 403 in production even
though sibling endpoints under `POST/ops/` may be permitted:

```yaml
declaration:
  override_ancestors: true

deny:
  status: 403
  return:
    error: "inject-fault is disabled in production"
  next: end
```

See [Guards](../guards.md) for the full override-vs-stacking rules.

## Java-Ruuter parity notes

- **`method`, `accepts`** — removed from the Rust struct (task 070).
  They were accepted-but-never-read in Ruuter-on-Rust prior; Java
  used them for documentation only. `method` is redundant with the
  DSL's parent directory (`GET/`, `POST/`, …); `accepts` is redundant
  with `Content-Type` handling.
- **`returns`** — repurposed. Was a string tag in Java (documentation
  only); is now a typed response-shape declaration
  (`Option<Vec<DslField>>`) that flows into the OpenAPI spec. Old
  string-shape `returns:` values are ignored (backwards-compat break
  from a field nothing read).
- **Rich `DslField` metadata** — Java Ruuter's `DslField` type
  reserved room for `type`, `required`, `format` but never wired
  them. Ruuter-on-Rust (task 070) wires them for OpenAPI generation.
- **`strict:` flag** — new in task 070. Java Ruuter has no per-DSL
  strict-keys posture; the framework always filters silently. Ruuter's
  default matches Java (filter-and-continue); the flag is opt-in.

## Cross-links

- [Guards](../guards.md) — `override_ancestors` semantics.
- [OpenAPI generation](../../framework/openapi.md) — how declaration
  fields shape the spec.
- [Configuration](../../ops/configuration.md) — the
  `dsl.warn_on_missing_declaration` toggle.
- [Logging: errors](../../logging/errors.md) — how the 400 from
  `strict:` renders in the log stream.

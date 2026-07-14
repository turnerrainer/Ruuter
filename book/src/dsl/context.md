# Context bindings

Every `${...}` expression sees a fixed set of bindings.

## Request-scoped

| Binding | Populated from | Present when |
|---------|----------------|--------------|
| `incoming.body`               | request body, parsed as JSON if Content-Type is `application/json` | always (may be `{}`) |
| `incoming.query`              | URL query string (all values as strings) | always |
| `incoming.params`             | alias of `incoming.query` | always |
| `incoming.params.pathParams`  | trailing URL segments stripped during route resolution | HTTP requests only |
| `incoming.headers`            | request headers (lower-cased keys → string values) | always |
| `incoming.connection_id`      | per-WS-client id like `client:<32-hex>` | WebSocket DSLs only |

## Non-JSON body

Requests without `Content-Type: application/json` produce an empty `incoming.body`. Malformed JSON on a JSON-typed request returns `400 Bad Request` before the DSL runs.

WebSocket text frames are parsed as JSON when possible. Non-JSON text arrives as `{ "value": "<text>" }`.

## User variables

Anything named by:

- `assign` step
- `state.get`'s `into:`
- `http.result`, `template.result`
- guard-side `assign` (visible to the main DSL that runs after the guard passes)
- `iterate.into`

Undefined names in `${...}` evaluate to `undefined`; use `??` to default:

```yaml
value: "${possibly_missing ?? 'fallback'}"
```

# Context bindings

Every `${...}` expression sees a fixed set of bindings.

## Request-scoped

| Binding | Populated from | Present when |
|---------|----------------|--------------|
| `incoming.body`               | request body, parsed as JSON if Content-Type is `application/json` | always (may be `{}`) |
| `incoming.params`             | URL query string (all values as strings) | always |
| `incoming.params.pathParams`  | trailing URL segments stripped during route resolution | HTTP requests only |
| `incoming.headers`            | request headers (lower-cased keys → string values) | always |
| `incoming.connection_id`      | per-WS-client id like `client:<32-hex>` | WebSocket DSLs only |

## Non-JSON body

Requests without `Content-Type: application/json` produce an empty `incoming.body`. Malformed JSON on a JSON-typed request returns `400 Bad Request` before the DSL runs.

WebSocket text frames are parsed as JSON when possible. Non-JSON text arrives as `{ "value": "<text>" }`.

## Query-parameter shape (last-wins)

`incoming.params` is a **flat map**: one string value per key. When a client sends the same key more than once —

```
GET /svc/search?tag=red&tag=blue&tag=green
```

— the DSL sees only **one** value under `${incoming.params.tag}`. Which one is undefined by the HTTP spec, but Ruuter's parser resolves duplicates by inserting into a `HashMap` in URL-order, so the **last value wins**: the DSL reads `${incoming.params.tag}` as `"green"`, not `"red"` or `["red","blue","green"]`.

This is a real footgun for two shapes:

- **Attacker-picked value.** If a DSL trusts `${incoming.params.tag}` for authorization or logging and the client can append their own copy of the key after one earlier in the URL, the client wins. Any DSL that keys a decision on a query parameter should either (a) treat the value as *user-controlled anyway*, or (b) refuse duplicate keys explicitly — for example by wrapping the DSL in a guard that inspects the raw URL and rejects `?tag=…&tag=…`.

- **Silent drop.** A DSL that expects "the first `tag` a client sent" (matching some clients that treat query params as an ordered list) reads the *last* one instead. If the two values differ, the DSL misbehaves without any obvious signal in logs.

The framework offers **no built-in helper** for reading duplicate query values as an array. If your API needs multi-value semantics, encode the multiplicity into the value shape (`?tags=red,blue,green` and split in the DSL via a JS expression) instead of relying on repeated keys. This mirrors the choice most HTTP frameworks make and avoids a Ruuter-specific primitive that would drift from wire realities.

WebSocket handshake query parameters go through the same parser and have the same last-wins semantic (see `handle_ws_upgrade` in `src/router/mod.rs`).

Origin: h2ck.me v1 T-32 (F-PR-4 in `BREAK-TESTS-OWASP-PROBES-v1`). Regression coverage in `tests/issue_T32_query_param_last_wins.rs`.

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

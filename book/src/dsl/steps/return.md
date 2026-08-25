# return

Send an HTTP response. Terminates the DSL run.

```yaml
respond:
  return:                       # required
    ok: true
    echo: "${incoming.body.value}"
  status: 202                   # optional; default 200
  headers:                      # optional
    X-Custom: "yes"
  next: end
```

- `return:` can be any JSON value: object, array, string, number, boolean, `null`.
- `status:` may be a literal `u16` or a JS expression that resolves to one.
- `headers:` map merges into the response. DSL-set values win over framework defaults. Two shapes accepted (see below).
- Framework always adds `traceparent` and `X-Trace-Id` on top (see [Response headers](../../framework/response-headers.md)).

## Dynamic `headers:` map

`headers:` accepts either a per-key mapping (each value may embed
`${…}`) or a single `${expr}` string that evaluates to a JSON
object at runtime:

```yaml
compute:
  assign:
    response_hdrs:
      X-Trace: "${incoming.headers['x-trace-id']}"
      X-Ratelimit-Remaining: "${remaining}"
  next: reply

reply:
  return: "ok"
  status: 200
  headers: "${response_hdrs}"     # whole-map expression
```

Runtime rules: the expression must evaluate to a JSON object (a
scalar / array is a step error); `null` = no headers. Issue #25
tracked adding this shape — previously only the per-key mapping
form parsed.

## Non-JSON responses (XML, HTML, plaintext)

By default, the response body is serialised through JSON — a
string return value comes out surrounded by double quotes with
special characters escaped, which is wrong for `text/xml` /
`text/html` / `text/plain` payloads.

To emit a raw response body, ALL of the following must hold:

1. `wrapper: false` on the return step (or `response.default_wrapper: false` in config).
2. The return value is a JSON string (not an object / array / number).
3. The DSL sets a non-JSON `Content-Type` header on the return step.

When all three hold, the framework bypasses `axum::Json` and
writes the raw string bytes with the DSL's Content-Type. Any
one condition missing → JSON path (back-compat preserved).

```yaml
respond:
  return: "<root><item>hello</item></root>"
  headers:
    Content-Type: "text/xml"
  wrapper: false
  status: 201
```

Response:

```http
HTTP/1.1 201 Created
content-type: text/xml
content-length: 31

<root><item>hello</item></root>
```

Issue #24 tracked this fix; before, the body came out as
`"<root>&#x2F;<item>hello&#x2F;<&#x2F;item>&#x2F;<&#x2F;root>"`
JSON-encoded regardless of the Content-Type header.

### Why the Content-Type gate?

Requiring an explicit non-JSON Content-Type is a deliberate
back-compat gate: DSLs that already used `wrapper: false` without
a Content-Type (interpreted as "no envelope, still JSON") keep
their existing shape. Only DSLs that opt into a non-JSON contract
via Content-Type get raw emission — the fix is opt-in per-return-
step, not a global behaviour change.

For structured (object / array) response bodies, JSON is still
the right shape — those cases stay on the JSON path regardless
of `wrapper:` and Content-Type.

## Bare-value response

```yaml
respond:
  return: pong
  status: 202
  next: end
```

Request:

```bash
curl -i http://localhost:8080/samples/ping
```

Response:

```http
HTTP/1.1 202 Accepted
content-type: application/json
xpingstatusheader: pong delivered

{"response":"pong"}
```

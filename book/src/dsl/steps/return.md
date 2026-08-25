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

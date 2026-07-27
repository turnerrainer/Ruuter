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
- `headers:` map merges into the response. DSL-set values win over framework defaults.
- Framework always adds `traceparent` and `X-Trace-Id` on top (see [Response headers](../../framework/response-headers.md)).

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

"pong"
```

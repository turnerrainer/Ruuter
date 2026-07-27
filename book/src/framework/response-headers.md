# Response headers

Every response carries the following, in addition to anything the DSL set explicitly:

| Header                  | Value | When |
|-------------------------|-------|------|
| `traceparent`           | W3C tracecontext, adopted from request or generated | always |
| `x-trace-id`            | 32-hex trace id extracted from `traceparent` | always |
| `access-control-*`      | as configured | `cors.allowed_origins` non-empty AND Origin matches |
| `content-type`          | `application/json` | always for framework-generated bodies |
| Anything under `response_default_headers` | as configured | always, unless DSL set the same header |
| Any header set by a `return` step's `headers:` | as-is | always (wins over defaults) |

## Verification

Request:

```bash
curl -sSD - -H 'Traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' \
     http://localhost:8080/samples/basic/hello -o /dev/null | grep -iE 'traceparent|x-trace-id|content-type'
```

Response header lines:

```http
content-type: application/json
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
x-trace-id: 4bf92f3577b34da6a3ce929d0e0e4736
```

## Merge precedence (highest wins)

1. `traceparent` and `x-trace-id` — always overwritten by the framework (you cannot spoof a trace id via the DSL).
2. `return.headers` from the DSL.
3. `content-type: application/json` — set by the JSON response body.
4. `response_default_headers` — added only if the same header name isn't already set by any of the above.

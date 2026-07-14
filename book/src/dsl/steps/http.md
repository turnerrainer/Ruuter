# http

Make an outbound HTTP request.

```yaml
fetch:
  call: http.get                 # http.{get,post,put,patch,delete}
  args:
    url: "https://api.example.com/orders/${id}"
    headers: { Authorization: "Bearer [#API_TOKEN]" }
    query:   { limit: 50 }
    body:    { note: "hi" }      # POST/PUT/PATCH; serialized as JSON
  result: upstream                # binds .response.{status,body,headers}
  timeout: 3000                   # ms; overrides default 15000
  next: reply
```

## Result shape

The bound variable is:

```json
{
  "response": {
    "status":  200,
    "body":    { ... }  // parsed JSON or null when body isn't JSON
    "headers": { "content-type": "application/json", ... }
  }
}
```

Reference downstream: `${upstream.response.status}`, `${upstream.response.body.field}`, `${upstream.response.headers['x-my-header']}`.

## Verbs

| Verb | Sends body |
|------|------------|
| `http.get`    | no |
| `http.post`   | yes |
| `http.put`    | yes |
| `http.patch`  | yes |
| `http.delete` | no |

## Framework behaviour

- `traceparent` is auto-forwarded on every outbound call — override by setting the header explicitly in `headers:`.
- URL and body are validated against the SSRF allow-list (see [SSRF allow-list](../../framework/ssrf.md)).
- Response body is capped at `http_response_size_limit`; over-cap = step error.
- Upstream status is filtered against `http_codes_allow_list` when non-empty; disallowed = step error.
- On network error / timeout: the step returns an error; the DSL response is `500` unless the DSL handles it via a wrapping guard.

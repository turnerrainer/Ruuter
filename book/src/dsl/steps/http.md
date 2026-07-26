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

## Runnable example

`DSL/samples/GET/http/simple-get.yml`:

```yaml
fetch_data:
  call: http.get
  args:
    url: "https://jsonplaceholder.typicode.com/users/1"
  result: api_response
  next: respond

respond:
  return:
    status: "success"
    data: ${api_response.response.body}
  next: end
```

```console
$ curl -s http://localhost:8080/samples/http/simple-get | jq .
{
  "data": {
    "address": {
      "city": "Gwenborough",
      "geo": { "lat": "-37.3159", "lng": "81.1496" },
      "street": "Kulas Light",
      "suite": "Apt. 556",
      "zipcode": "92998-3874"
    },
    "company": {
      "bs": "harness real-time e-markets",
      "catchPhrase": "Multi-layered client-server neural-net",
      "name": "Romaguera-Crona"
    },
    "email": "Sincere@april.biz",
    "id": 1,
    "name": "Leanne Graham",
    "phone": "1-770-736-8031 x56442",
    "username": "Bret",
    "website": "hildegard.org"
  },
  "status": "success"
}
```

Requires outbound internet + the target host on the SSRF allow-list
if you've enabled the allow-list.

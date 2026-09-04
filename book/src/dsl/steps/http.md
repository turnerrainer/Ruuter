# http

Make an outbound HTTP request.

```yaml
fetch:
  call: http.get                 # http.{get,post,put,patch,delete}
  args:
    url: "https://api.example.com/orders/${id}"
    headers:
      Authorization: "Bearer [#API_TOKEN]"
    query:
      limit: 50
    body:                        # POST/PUT/PATCH; serialised as JSON
      note: "hi"
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
    "body":    { ... },  // see below
    "headers": { "content-type": "application/json", ... }
  }
}
```

Reference downstream: `${upstream.response.status}`, `${upstream.response.body.field}`, `${upstream.response.headers['x-my-header']}`.

### Response body decoding

- **JSON** upstream (any body that parses as JSON) → parsed value
  (object / array / number / string / bool / null).
- **Non-JSON** upstream (XML, HTML, plaintext, or any body that
  fails JSON parse) → the raw text as a string, accessible via
  `${upstream.response.body}`. UTF-8 lossy: invalid byte
  sequences render as U+FFFD rather than failing the step.
- **Empty** body (`content-length: 0` or a chunked response with no bytes) → `""` (empty string), matching Java Ruuter and the wire truth. A DSL that forwards `${upstream.response.body}` as a plaintext outbound body sends the same empty payload it received — not the string `"null"`. Prior to issue #63, empty bodies bound as JSON `null`, which surfaced downstream as `"null"` when re-serialised as plaintext.

Before issue #23 was fixed, non-JSON responses silently became
`null`, losing the payload — an XML mapper couldn't return XML,
a plaintext error message from an upstream disappeared, etc. The
fallback-to-string behaviour lets DSLs forward or inspect non-JSON
upstreams without special-casing.

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

## Dynamic `headers:` / `query:` maps

Both `headers:` and `query:` accept two shapes:

**Per-key mapping** (traditional; each value may embed `${…}`):

```yaml
args:
  headers:
    Authorization: "Bearer ${token}"
    X-Trace-Id:    "${incoming.headers['x-trace-id']}"
```

**Whole-map expression** — a single `${expr}` string that evaluates
to a JSON object at runtime. Useful when the map is computed by
merging / spreading upstream:

```yaml
compute:
  assign:
    merged_headers: "${Object.assign({}, upstream.response.body[0].headers, { 'Content-Type': 'application/json' })}"
  next: forward

forward:
  call: http.post
  args:
    url: "[#REMOTE_SERVICE_URL]"
    headers: "${merged_headers}"     # ← whole-map expression
    body: "${incoming.body}"
```

Runtime rules for the whole-map form:

- The expression MUST evaluate to a JSON object. Anything else
  (array, scalar, `null`) is a step error with a diagnostic
  naming the field. `null` is treated as "no headers".
- Individual values inside the resulting object are used verbatim
  (no second-pass `${…}` evaluation — do the interpolation inside
  the expression).
- The framework still auto-forwards `traceparent` unless the
  evaluated map contains it.

Prior to v0.9, only the per-key mapping shape was accepted —
`headers: "${expr}"` failed at DSL load time with
`invalid type: string, expected a map`. Issue #25 tracked the fix.

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

Request:

```bash
curl -s http://localhost:8080/samples/http/simple-get | jq .
```

Response:

```json
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

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

Issue #98: decoding is driven by the response `Content-Type` header,
not by attempting a speculative JSON parse. The same rules apply on
every transport — TCP, `unix://` URLs, and the `unix_socket_map` host
aliases.

| Response `Content-Type`                     | `${…response.body}` type    |
|---------------------------------------------|-----------------------------|
| `application/json` (with or without `; …`)  | parsed JSON value           |
| `application/*+json` (`problem+json`, `hal+json`, `ld+json`, `vnd.api+json`, …) | parsed JSON value |
| anything else (`text/*`, `application/xml`, `image/*`, `application/octet-stream`, …) | UTF-8 lossy string |
| missing header                              | UTF-8 lossy string          |
| empty body (any Content-Type)               | `""` (empty string)         |

Notes:

- **Content-Type matching is case-insensitive on both the type and
  the `+json` subtree** (RFC 9110 §8.3.1). Media-type parameters
  (`; charset=utf-8`, `; q=…`) are tolerated.
- **UTF-8 fallback is lossy** — invalid byte sequences render as
  U+FFFD rather than failing the step. Applies to every non-JSON
  path.
- **`Content-Type: application/json` with a body that fails to
  parse** (a gateway-502 pattern where the proxy returns HTML with
  a lying Content-Type) logs a WARN naming the parse error and
  binds the raw text as a string — so the DSL can still forward /
  inspect and emit a semantic 502 without the step raising.
- **Empty body** binds `""` (not `null`) regardless of
  `Content-Type`, preserving the issue #63 fix. A DSL that
  forwards `${upstream.response.body}` as a plaintext outbound
  sends the same empty payload it received — never the four-byte
  string `"null"`.

Behaviour change vs pre-#98 releases: a `text/plain` or
missing-`Content-Type` response whose body happens to be valid JSON
(`123`, `null`, `true`, `"hello"`, `{"a":1}`) used to be parsed and
reach the DSL as a JSON number / null / bool / string / object. It
now stays as the raw text, matching the wire declaration. Two
migration paths for DSLs that relied on the old byte-heuristic:

- Preferred: fix the upstream to send
  `Content-Type: application/json`.
- Otherwise: `${JSON.parse(r.response.body)}` in the DSL.

Historical context: before issue #23 was fixed, non-JSON responses
silently became `null`, losing the payload — an XML mapper couldn't
return XML, a plaintext error message from an upstream disappeared,
etc. The #23 fix added a string fallback on the TCP path. The #98
fix generalised that fallback to all three transports (TCP was still
guessing rather than reading the header; UDS silently discarded
non-JSON) and put `Content-Type` in charge of the decode.

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
- **Transport failure** (connection refused, DNS, TLS handshake, read/write timeout, mid-body read error) — issue #89: the step binds an in-band stub `{response: {status: 0, error: "<kind>", body: {error, message}, headers: {}}}` under `result:` instead of raising. The DSL can then branch on `${result.response.status == 0}` (or on the specific kind via `${result.response.error == 'timeout'}`) in a subsequent `check_*` switch, or wire an `error:` handler on the step. Stable kinds: `timeout`, `connect`, `request`, `body`, `decode`, `unknown`. Policy-level pre-flight rejections (SSRF, host-allowlist, malformed URL, response-size cap) still raise — those are ops decisions, not availability events.

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

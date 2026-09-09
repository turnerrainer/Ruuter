# Failure modes

Every HTTP status the framework itself can emit, and what it means.

| Status | Body pattern | Cause |
|--------|--------------|-------|
| `400`  | `{"error": "invalid JSON body: …"}` | `Content-Type: application/json` with malformed body |
| `400`  | `{"error": "body read error: …"}`   | Body exceeded 16 MiB inbound cap |
| `403`  | `{"error": "CSRF: origin not allowed"}` | CSRF Origin/Referer check failed |
| `404`  | `{"error": "Not Found"}` | No DSL matched (even after path-param stripping) |
| `405`  | `{"error": "Method Not Allowed"}` | Method not in `incoming_requests.allowed_method_types` |
| `428`  | `{"error": "If-Match header is required for this method"}` | Optimistic-concurrency check failed |
| `500`  | `{"error": "Invalid DSL step: Unknown HTTP method: http.<xxx>"}` | DSL uses an unsupported `http.<verb>` |
| `500`  | `{"error": "Script evaluation error: …"}` | JS runtime error in a `${...}` expression |
| `500`  | `{"error": "HTTP request rejected: outbound HTTP is disabled …"}` | `internal_requests.disabled: true` |
| `500`  | `{"error": "HTTP request rejected: url not in internal_requests.allowed_urls: …"}` | SSRF allow-list |
| `500`  | `{"error": "HTTP request rejected: url host '<h>' not in internal_requests.allowed_ips"}` | SSRF IP allow-list |
| `500`  | `{"error": "HTTP request rejected: upstream response body … exceeds http_response_size_limit …"}` | Response size cap |
| `500`  | `{"error": "HTTP request rejected: upstream status … not in http_codes_allow_list"}` | Upstream status filter |
| `500`  | `{"error": "template not found: …"}` | `template` step target missing |
| `500`  | `{"error": "ws_send: no such connection '…'"}` | `ws_send` to an unknown connection id |
| `500`  | `{"error": "ws_send: no `to`, no `broadcast_prefix`, and context has no connection_id"}` | `ws_send` in an HTTP DSL without addressing |
| `500`  | `{"error": "File not found: DSL not found: …"}` | Should be 404 — surfaced when the router's exact-key lookup fails but the `execute_dsl` path was taken directly (test path only) |
| `500`  | `{"error": "Configuration error: …"}` | Constants file / config file / source config error at boot time |

Guards may emit ANY 4xx/5xx status — those come from the guard DSL's own `return.status`, not the framework.

## `http.*` transport failures do NOT produce a framework 500 (issue #89)

Connection refused / DNS / TLS handshake / read-write timeout / mid-body read on an `http.*` step no longer aborts the run. The step binds an in-band stub under `result:`:

```json
{
  "response": {
    "status":  0,
    "error":   "<kind>",
    "body":    {"error": "<kind>", "message": "<full reqwest source chain>"},
    "headers": {}
  }
}
```

Stable kinds: `timeout`, `connect`, `request`, `body`, `decode`, `unknown`. DSL branches on `${result.response.status == 0}` or `${result.response.error == 'timeout'}`, or wires an `error:` handler on the step. Whatever status the DSL then emits becomes the framework response — 502 is the typical shape for a gateway/adapter DSL. See [`http` step docs](../dsl/steps/http.md#framework-behaviour).

## Cache-hit replay

An [Idempotency-Key](../framework/idempotency.md) cache hit replays the cached response verbatim — same status, same body. The response gains `Idempotency-Replayed: true`. Neither the guard chain nor the main DSL re-runs.

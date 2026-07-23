# Request pipeline

Order of framework checks per HTTP request. Each stage can short-circuit with the noted status.

1. **WebSocket upgrade detection**. `Upgrade: websocket` + `GET` → hand off to WS handler; skip remaining stages.
2. **Method allow-list**. Method not in `incoming_requests.allowed_method_types` → `405 Method Not Allowed`.
3. **CSRF Origin check**. `csrf.allowed_origins` non-empty AND method ∈ `csrf.enforce_on_methods` AND Origin/Referer not allowed → `403 Forbidden`. Skipped if `allowed_origins` is empty.
4. **If-Match presence**. `optimistic_concurrency.require_if_match: true` AND method ∈ `enforce_on_methods` AND no `If-Match` header → `428 Precondition Required`.
5. **Body read + JSON parse**. Body over 16 MiB → `400`. `Content-Type: application/json` + malformed body → `400 Bad Request`. Non-JSON content types produce empty `incoming.body`.
6. **Origin resolution**. `X-Forwarded-For` (or `X-Real-IP`) is promoted into `incoming.origin` only when the direct TCP peer's IP is in `proxy.trusted`; otherwise `origin` reflects the socket peer. Raw headers remain visible in `incoming.headers`.
7. **Route resolution**. Exact `<METHOD>/<path>` lookup. On miss: path-param stripping. No match → `404 Not Found`.
8. **Guard chain**. All applicable guards (outermost-first, unless an override guard matches). Any guard returning status ≥ 400 → that response, skip stage 9.
9. **Main DSL execution**.
10. **Response assembly**:
    - DSL-set headers merged first.
    - `traceparent` echoed (adopted from request or generated fresh).
    - `X-Trace-Id` extracted from traceparent.
    - `Access-Control-*` added when CORS is configured and Origin matches.
    - `response_default_headers` merged last, without overwriting anything set above.

Framework-level `Idempotency-Key` handling was removed in v0.7.0
(h2ck.me findings S1 + S5). See [Idempotency pattern](../dsl/idempotency-pattern.md)
for the DSL-authored replacement.

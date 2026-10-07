# Request pipeline

Order of framework checks per HTTP request. Each stage can short-circuit with the noted status.

1. **WebSocket upgrade detection**. `Upgrade: websocket` + `GET` → hand off to WS handler; skip remaining stages.
2. **Method allow-list**. Method not in `incoming_requests.allowed_method_types` → `405 Method Not Allowed`.
3. **CSRF Origin check**. `csrf.allowed_origins` non-empty AND method ∈ `csrf.enforce_on_methods` AND Origin/Referer not allowed → `403 Forbidden`. Skipped if `allowed_origins` is empty.
4. **If-Match presence**. `optimistic_concurrency.require_if_match: true` AND method ∈ `enforce_on_methods` AND no `If-Match` header → `428 Precondition Required`.
5. **Pass-through proxy early-dispatch** (issue #134). Route resolves to a DSL with `declaration.proxy:` → run guards against a **header-only** `ExecutionContext` (empty `incoming.body`) → stream the request body to the upstream and the response back, byte-identical. Bypasses stages 6–12 of this pipeline (internal gate, body parse, body dispatch, main DSL execution). The route's per-route caps apply: Content-Length preflight against `max_body_bytes`, Content-Encoding allowlist, Semaphore cap (`max_in_flight`), inbound idle timeout, overall `request_timeout_ms`. See [Pass-through proxy routes](../dsl/proxy.md).
6. **Internal-DSL gate** (issue #143). Resolve `(method, path)` to a candidate DSL and compute `effective_internal` via the three-level fallback (per-DSL → `declarations.default_internal` → framework default `false`). If `true`, return `404 Not Found` with the standard body (not `403` — avoids leaking that the route exists). Runs BEFORE body read so a flood of oversized requests to an internal route never reads a byte off the socket. `template:` and self-call-shortcircuit paths bypass this handler entirely and reach internal DSLs normally. See [Internal-only DSLs](../dsl/internal-dsls.md).
7. **Body read + JSON parse** (non-proxy routes only). Body over 16 MiB → `400`. `Content-Type: application/json` + malformed body → `400 Bad Request`. Non-JSON content types produce empty `incoming.body`.
8. **Origin resolution**. `X-Forwarded-For` (or `X-Real-IP`) is promoted into `incoming.origin` only when the direct TCP peer's IP is in `proxy.trusted`; otherwise `origin` reflects the socket peer. Raw headers remain visible in `incoming.headers`.
9. **Route resolution**. Exact `<METHOD>/<path>` lookup. On miss: path-param stripping. No match → `404 Not Found`.
10. **Guard chain**. All applicable guards (outermost-first, unless an override guard matches). Order: project-level `.guard.yml` (issue #39, if present) → method-scoped ancestors (outermost-first) → target. Any guard returning status ≥ 400 → that response, skip stage 11.
11. **Main DSL execution**.
12. **Response assembly**:
    - DSL-set headers merged first.
    - `traceparent` echoed (adopted from request or generated fresh).
    - `X-Trace-Id` extracted from traceparent.
    - `Access-Control-*` added when CORS is configured and Origin matches.
    - `response_default_headers` merged last, without overwriting anything set above.

Framework-level `Idempotency-Key` handling was removed in v0.7.0
(h2ck.me findings S1 + S5). See [Idempotency pattern](../dsl/idempotency-pattern.md)
for the DSL-authored replacement.

## Background execution (issue #137)

The [`detach` step](../dsl/steps/detach.md) runs its `do:` block in a
`tokio::spawn` task. After stage 10 (response assembly) emits the
parent's response, detached tasks continue in the background. The
SIGTERM drain in `main.rs` waits for them up to
`detach.shutdown_grace_secs` (default 15 s) before aborting. Each
detached task owns a context snapshot — writes made inside `do:`
don't propagate to the parent, and the parent's writes after
`detach` fires don't propagate into the detached task.

# 031 — End-to-end tests for the 0.4.0 security surfaces

## Why

The 0.4.0 hardening pass added CSRF Origin/Referer check, SSRF
allow-list, Idempotency-Key cache, response-size cap, traceparent
adoption/echo, method allow-list rejection. Unit tests exist for the
modules; nothing hits them through a live `axum::serve` listener,
so a regression in `router::handle_request`'s wiring order would
pass CI.

## Acceptance

New `tests/security.rs` with tokio-tests spinning the router on a
random local port:

- CSRF allowed-origins non-empty + POST from disallowed Origin → 403.
- CSRF allowed-origins empty + POST from any Origin → 2xx (bypassed).
- SSRF `internal_requests.disabled=true` → an `http.*` step returns
  `HttpRequest("outbound HTTP is disabled …")`.
- SSRF `allowed_urls=["https://example.com"]` + `http.get` to
  `https://other.example.com` → step fails; to allowed prefix → 200.
- Idempotency-Key: two consecutive POSTs with the same key produce
  identical responses and the second has `Idempotency-Replayed: true`.
- Response-size cap: 1 MiB cap + upstream returning 2 MiB (via
  mockito) → step fails with the expected error, no OOM.
- Traceparent: request with `traceparent: 00-<id>-<span>-01` gets
  the same trace id in the response `X-Trace-Id` header.
- Method allow-list: `allowed_method_types=["GET"]` + `POST /foo`
  → 405.

Uses `mockito` (already in dev-dependencies, currently unused).

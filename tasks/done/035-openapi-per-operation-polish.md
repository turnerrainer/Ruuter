# 035 — Reduce OpenAPI spec's Redocly warnings toward zero

## Why

The auto-generated spec at `GET /_/openapi.json` validates cleanly
(0 errors) but Redocly emits 33 warnings — mostly
`operation-4xx-response` and `operation-2xx-response`
("operation should document at least one 4xx / 2xx response"). A
partner-facing rendered spec (Redocly, Swagger UI, Stoplight) looks
under-specified. Root cause: `collect_return_statuses` only picks
literal `status:` values from `return` steps, not the ones a `switch`
case might set.

## Acceptance

- `openapi::collect_return_statuses` walks `switch` branches and the
  headers/status a nested guard could emit, not just top-level
  `return` steps.
- Every operation lists at least one 2xx and at least one 4xx (fall
  back to 500 if no 4xx is inferrable — every route can produce one
  via the `Err(_) => INTERNAL_SERVER_ERROR` catch-all in
  `handle_request`).
- Every operation carries a short auto-derived `description` (e.g.
  the DSL's summary + the reserved status codes).
- Redocly lint on the produced spec: `0 errors, ≤ 5 warnings`.

# 033 — Acceptance test for `http.patch` DSL step

## Why

Task 023 was closed by adding one line to `HttpStepExecutor::parse_method`,
but its own acceptance criteria included:

> Add `tests/dsl/http_methods/test_http_patch.yml` covering: 200,
> 4xx, 5xx, network timeout

That test does not exist. The 2026-06-29 outage that filed 023
happened because runtime behaviour disagreed with the DSL author's
mental model — closing the ticket without the test that would have
caught the original bug leaves the same regression class open.

## Acceptance

- `tests/http_patch.rs` (mockito-backed) with cases:
  - 200 OK + JSON body → step succeeds, `result.response.status == 200`.
  - 4xx (e.g. 409 Conflict) with JSON error body → step succeeds
    with the 409 preserved in `result.response.status`.
  - 5xx from upstream → step succeeds with the 5xx preserved.
  - Configured timeout (`timeout: 100` ms) + upstream that sleeps
    500 ms → step returns `Err(Http(reqwest::Error::…))` with a
    timeout classification.
- All four cases prove the outgoing HTTP method is literally `PATCH`
  (assert against mockito's recorded request).

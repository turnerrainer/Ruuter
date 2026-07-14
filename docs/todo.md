# Ruuter-RS state of play

**Current version:** 0.4.0 · **Last refreshed:** 2026-07-14.

## What ships today

- File-system-based REST routing (`DSL/<project>/<METHOD>/<path>.yml`).
- WebSocket server endpoints (`DSL/<project>/WS/<path>.yml`) with
  per-connection identity and `ws_send` for replies / fan-out.
- WebSocket sources (`DSL/<project>/sources/<name>.yml`) → trigger
  dispatch (`DSL/<project>/triggers/<channel>/<key>.yml`) under a
  restarting source supervisor.
- Step types: `assign`, `return`, `http` (get/post/put/patch/delete),
  `switch`, `log`, `state`, `iterate`, `ws_send`; `template` is a
  placeholder (see task 027).
- Guards (`<stem>.guard.yml`) — per-directory pre-execution DSLs.
- JavaScript expression evaluation (Boa) with runtime limits.
- `constants.ini` `[#KEY]` substitution.
- OpenAPI 3.1 spec auto-generated from every DSL at
  `GET /_/openapi.json`.
- Framework security surface (all configurable via `ruuter.yaml`):
  CSRF Origin/Referer allow-list, Idempotency-Key cache, SSRF
  allow-list, response-size cap, upstream status filter, method
  allow-list, response-default-headers, CORS.
- W3C traceparent adoption/echo + `X-Trace-Id` on every response;
  auto-forwarded on outbound HTTP calls. OTel OTLP export opt-in.
- Docker: multi-stage build, `tini`, non-root, `read_only`,
  `no-new-privileges`, `cap_drop: ALL`, mem/cpu limits.

## Roadmap

The actual roadmap is the numbered tickets under `tasks/`:

- `tasks/backlog/` — filed, unstarted.
- `tasks/in-progress/` — being worked.
- `tasks/in-review/` — implementation landed, awaiting review.
- `tasks/acceptance-testing/` — reviewed, awaiting test sign-off.
- `tasks/done/` — completed.
- `tasks/blocked/` — filed but held pending an upstream decision.

If a piece of work isn't in one of those queues, it's not planned.

## Design invariants (do NOT drift)

- Ruuter is a dumb pipe. It routes, guards, and translates — it does
  not do IAM, does not fetch secrets, does not know about Resql
  schemas. Cross-component knowledge belongs in the DSL, not the
  framework.
- Every response gets `traceparent` + `X-Trace-Id` for correlation.
- Every framework-enforced hardening (CSRF, SSRF, size cap, method
  filter) has a safe default so an operator who ignores the config
  gets sensible behaviour.

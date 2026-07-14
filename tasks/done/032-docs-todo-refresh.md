# 032 — Refresh `docs/todo.md` to reflect current state

## Why

The file still describes "Phase 1: Foundation (v0.1.x)" as pending
work with checkboxes on `Core DSL parser with serde_yaml`, `Basic
HTTP server with Axum`, etc. — all shipped in v0.2. A new contributor
lands on that doc and gets a false picture of maturity.

## Acceptance

- Rewrite as a short "Where things stand at 0.4.0" section (steps,
  guards, WS, sources, OpenAPI, security surfaces — all done).
- Point at `tasks/backlog/` as the actual roadmap.
- Delete the aspirational Phase 4–8 checkbox lists; anything still
  wanted moves into `tasks/backlog/` as a numbered ticket.

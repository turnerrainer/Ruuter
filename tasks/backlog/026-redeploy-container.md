# 026 — Redeploy the running container onto the fresh image

## Why

`ruuter-on-rust:0.4.0` was rebuilt this cycle but the local
`ruuter-on-rust` container may need a `docker compose up -d
--force-recreate` after each round of source changes. Ship-ready
means "the process people can `curl` matches the image people can
build."

## Status

**Excluded from this batch by owner** — will be done after every other
open backlog item lands (owner wants a single redeploy, not one per
change).

## Acceptance

- `docker compose up -d --force-recreate` succeeds.
- Healthcheck goes to `healthy` within `start_period`.
- `curl localhost:8080/_/openapi.json` returns the current spec.
- Old container removed, new one running the freshly-built image.

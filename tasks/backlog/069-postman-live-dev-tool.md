# 069 — Postman collection as a live dev tool

## Filed

2026-07-26 — surfaced while writing the Getting Started book chapters
that introduce the Postman assets to first-time users.

## Severity

**Medium** — quality-of-life for DSL authors; nothing broken today.
Blocks the Postman collection being genuinely useful past a single
import.

## Symptoms

Today's Postman assets live under `postman/` and cover only
`DSL/samples/`, not `DSL/*`. Regeneration is a two-line manual recipe:

```bash
curl -s http://localhost:8080/_/openapi.json > postman/openapi.json
npx openapi-to-postmanv2 -s postman/openapi.json \
    -o postman/ruuter.postman_collection.json -p
```

Two problems:

1. **Scope.** DSLs under `DSL/<any-project>/*` are ignored — the
   committed collection is samples-only. In practice a working
   deployment has `samples/` plus one or more real projects; the
   collection should show all of them so the dev can hit any
   endpoint out of the box.
2. **Re-import friction.** After regeneration, the developer has to
   go into Postman → File → Import → drop the regenerated file
   → confirm-replace. Ruuter has no way to push updates *to* the
   Postman workspace; the Postman API works the other way (Postman
   pulls from a URL or a git repo).

## Proposal (both parts)

### Part A — collection covers `DSL/*`, not `DSL/samples/*`

The recipe already reads from `/_/openapi.json`, which is generated
from every DSL under `config.config_path`. So *scope-wise* the
committed collection is only samples-only because the repo happens to
ship only `DSL/samples/`. Nothing to fix in the tool — but the docs
need to make clear that:

- The collection reflects whatever `DSL/*` was live when the
  developer last ran the regeneration recipe.
- Adding a project = add a DSL under `DSL/<project>/`, restart or
  hot-reload, re-run the recipe.

### Part B — live-updating collection

Two workable shapes; pick one:

1. **`/_/postman.json` endpoint** — Ruuter serves a Postman v2.1
   collection JSON alongside `/_/openapi.json`, generated from the
   same in-memory tree. Postman can then import from URL:
   `File → Import → Link → http://localhost:8080/_/postman.json`.
   Postman auto-refreshes URL imports periodically; the developer
   gets updated endpoints without touching the workspace. Trade-off:
   Ruuter now ships a second spec formatter. Rough dependency:
   `openapi-to-postmanv2` is Node-only, so this either means shelling
   out (fragile in the container) or reimplementing enough of the
   OpenAPI → Postman transform in Rust. The subset actually used is
   small — path, method, header, query, one example request per
   operation — so a bespoke Rust converter is realistic (~500 LOC).

2. **Sidecar container `ruuter-postman-refresher`** — small container
   in the compose file that polls `/_/openapi.json`, runs
   `openapi-to-postmanv2`, and writes the result to a shared volume
   the developer mounts into their Postman workspace. Sidesteps the
   Rust conversion; adds ops surface.

Option 1 is preferred (integrated, no sidecar) if the Rust converter
stays under ~800 LOC and passes a golden-file test suite against the
existing `openapi-to-postmanv2` output.

## Explicit non-goals

- Pushing collection updates into Postman's cloud workspace via
  their API — that requires a Postman API key per developer, which
  is not something Ruuter should be in the business of managing.
- Live-updating the environment file (`baseUrl`). That's a one-time
  configuration.

## Effort estimate

- Docs clarification (Part A): ~1 hour.
- `/_/postman.json` endpoint with bespoke converter (Part B option 1):
  ~1-2 days including golden-file tests against `openapi-to-postmanv2`
  output for the current sample corpus.
- Sidecar container (Part B option 2): ~half a day.

## Related

- `postman/README.md` — current regeneration recipe.
- `book/src/getting-started/postman.md` — the first-time-user
  walkthrough that would benefit most from a live-updating collection.
- OpenAPI generation lives in `src/openapi.rs` — the natural home for
  a `build_postman_from_http()` sibling.

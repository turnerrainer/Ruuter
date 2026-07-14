# 020 — Bespoke per-endpoint guard overrides

## Why

Today (and on the Java original) `applicable_guards` stacks every
ancestor guard along the path: a guard at `<METHOD>/<dir>` runs for
every DSL whose key starts with `<METHOD>/<dir>/`, and a deeper-nested
guard on the same lineage runs in addition. All must pass.

Consumers want a different model when a specific endpoint has
different privilege than its siblings: declare a guard at the
endpoint level and have it **replace** (not stack with) the folder-
level guard. This is the standard nginx `location` / Spring
filter-chain semantics — closest-match wins.

## Use case (desk)

desk has folder-level guards like `POST/test/.guard.yml` requiring
`X-Internal-Caller`. The `inject-fault` endpoint inside the same
folder is stricter (no production access at all). The current model
forces inject-fault's guard to layer on top of test/'s — duplication
+ ordering concerns. Override semantics let `POST/test/inject-fault.guard`
fully replace the folder-level guard with a stricter check.

## Proposed semantics

1. Continue to support the existing stack model as the default.
2. Add an opt-in marker — most natural is a top-level field in the
   bespoke guard DSL: `override_ancestors: true`. When present and
   true, `applicable_guards` returns only this guard for any DSL
   whose key matches it; ancestor guards are skipped for that
   request.
3. Multiple bespoke overrides: most-specific (longest key) wins.

Without the marker, behavior is identical to today (back-compat).

## Bespoke guard placement

Two valid placements, both supported:

- **In-folder** (Java parity, blocked on task #019): a guard file
  inside the endpoint's own folder. URL `/foo/bar` → guard at
  `POST/foo/bar/.guard.yml` (requires bar/ to be a folder, not a
  flat file — that's the natural shape when an endpoint already
  sits inside a per-resource folder like `orders/id.yml`).
- **Sibling** (Rust-port-only convention): `POST/foo/bar.guard.yml`
  next to `POST/foo/bar.yml`. Works today; adopts override semantics
  via the same `override_ancestors` field.

## Files likely touched

- `src/router/mod.rs` — `applicable_guards` returns only the most-
  specific override-marked guard (and its descendants past it? no
  — just it) when one matches.
- `src/dsl/...` — add the `override_ancestors` field to the DSL
  schema; it's a top-level marker, parsed once at load time.

## Tests

- Endpoint with folder guard + sibling override guard (override=true):
  only override runs.
- Endpoint with folder guard + sibling override guard (override=false):
  both run (back-compat).
- Multiple ancestor guards + one override deeper in the path:
  only override runs.
- No override anywhere: current stack behavior unchanged.

## Out of scope

- Per-route guard inheritance customization beyond "replace all
  ancestors". If a consumer wants "skip one specific ancestor",
  they can structure their DSL tree to put that ancestor elsewhere.

## Acceptance

A folder-level guard requires a basic header; a single endpoint
inside the folder demands a stricter token. Only the strict guard
runs for that endpoint; siblings keep using the folder-level guard.

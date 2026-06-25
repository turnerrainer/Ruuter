# 015 — Implement the guards primitive (currently a placeholder)

**Status**: BACKLOG.
**Severity**: MEDIUM-HIGH (production blocker for any DSL that needs
auth — e.g. CronManager-driven scheduled jobs, TIM JWT verification,
internal-only routes).
**Effort**: 1-2 days.
**Filed**: 2026-06-17.
**Related**: #013 (CronManager integration needs guards for the
shared-secret pattern).

## What's wrong

`README.md` lists guards as `⚠️ Guards system (placeholder)`. The
`src/guards/` module exists but does nothing. Java Ruuter supports
guards as a `.guard.yml` sibling file that runs before the main DSL
and can short-circuit with a non-2xx response (used for auth checks,
input validation, allowlist filtering).

In Buerostack, guards are how:
- TIM-issued JWTs get verified at the gateway
- Internal-only routes are restricted to known callers (e.g.
  `X-Internal-Caller` from CronManager)
- IP allowlists are applied
- Rate limits are enforced per-route

Without guards, every service that needs auth has to either skip it
(unsafe) or bake auth checks into the main DSL flow (mixes business
logic with security; violates the defense-in-depth principle).

## Fix

1. Re-enable the `.guard.yml` discovery path the loader already
   skips (`src/dsl/loader.rs:107` — `is_guard_file`).
2. Before executing the main DSL, run the sibling guard DSL (same
   step engine, same context). If the guard returns a non-2xx
   status or sets a "blocked" flag, short-circuit and respond
   with that status — main DSL doesn't run.
3. Expose a `guards` step type (or just rely on existing `switch` +
   `return`) for early termination.
4. Document the convention in a top-level guide:
   `docs/how-to/write-a-guard.md`.

## Verification

- `samples/protected.guard.yml` already exists in `DSL/samples/GET/`
  — make it actually enforce its check. Add an integration test:
  GET `/samples/protected` without the required header → 401; with
  it → 200.
- Round-trip with #013's CronManager pattern: a route with a
  guard that requires `X-Internal-Caller: cronmanager` rejects
  outside requests, accepts CronManager calls.

## Why this is generic

Auth/validation is a cross-cutting concern, not service-specific.
Every Buerostack component that exposes HTTP needs the same
primitive.

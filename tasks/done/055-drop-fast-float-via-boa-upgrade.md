# 055 — Drop `fast-float 0.2.0` by upgrading `boa_engine` (h2ck.me S8b)

## Filed

2026-07-20 — surfaced by h2ck.me pre-publication audit (finding
S8b, `REVIEW.md`), still open after the 2026-07-19 fix batch.
Pinned by
`tests/security_hardening.rs::cve_floor_cargo_lock_contains_known_bad_versions`.

## Severity

**High** — `RUSTSEC-2025-0003` is a segfault-on-input advisory
against `fast-float 0.2.0` for which "No fixed upgrade is
available." The crate is pulled transitively via `boa_engine 0.19`
→ `boa_parser` / `boa_string`. Any Boa `${...}` expression that
lexes a number-shaped literal from caller-influenced input can
panic the process.

## Problem

`Cargo.toml:19` pins `boa_engine = { version = "0.19", optional = true }`.
`Cargo.lock` shows three sites depending on `fast-float 0.2.0`
(direct + transitive via `boa_engine`, `boa_parser`, `boa_string`).

Also flagged by `cargo audit`:

- **RUSTSEC-2024-0379** — multiple soundness issues in `fast-float`.

The advisory has no fix path in `fast-float` itself. The remediation
is to remove the dependency. Recent Boa releases (0.20+) replaced
`fast-float` with `lexical`.

## Fix

Bump `boa_engine` to the latest stable release that no longer
depends on `fast-float`. As of writing that's 0.20 or later.

```toml
# Cargo.toml
- boa_engine = { version = "0.19", optional = true }
+ boa_engine = { version = "0.20", optional = true }   # or later
```

Verify:

```bash
cargo tree -i fast-float   # should print "no matching package"
```

## Acceptance

- `cargo tree -i fast-float` returns "no matching package".
- `cargo audit` no longer reports RUSTSEC-2025-0003 or
  RUSTSEC-2024-0379.
- All Boa-backed scripting tests pass:
  `tests/scripting_037_literal_fastpath.rs`,
  `tests/scripting_045_expr_registry.rs`, plus every test in
  `tests/security_hardening.rs::scripting_*`.
- Remove `("fast-float", "0.2.0", ...)` from `KNOWN_BAD` in
  `tests/security_hardening.rs::cve_floor_cargo_lock_contains_known_bad_versions`.

## Risk

Boa's public API has changed between 0.19 → 0.20 (context init,
some `JsValue` constructors, source-parsing entry points). Expect
some churn in `src/scripting/boa.rs`. If a Boa upgrade path is
blocked by API breakage in a hot path (e.g. `context.get_all_variables`
signature change), the fallback is to vendor a patched `fast-float`
locally with the missing bound check — but that's a maintenance
liability; prefer the Boa bump.

Second risk: perf regression. The v0.6.5 numbers in CHANGELOG.md
depend on Boa's current behaviour. Re-run the `bench/` A/B harness
after the bump. If throughput drops materially, file a follow-up
task; do not block the security fix.

## Cross-reference

- `projects/Ruuter-on-Rust/REVIEW.md § S8`
- `projects/Ruuter-on-Rust/REMEDIATION.md § S8b`
- Related: task 054 (serde_yml swap), task 056 (`cargo audit` CI
  gate). Ship all three as one supply-chain cleanup PR.

Effort estimate: 30 min for the bump + full test run, plus
whatever API churn shows up.

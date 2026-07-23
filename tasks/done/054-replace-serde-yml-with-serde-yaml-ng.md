# 054 — Replace `serde_yml` with `serde_yaml_ng` (h2ck.me S8a)

## Filed

2026-07-20 — surfaced by h2ck.me pre-publication audit (finding
S8a, `REVIEW.md`), still open after the 2026-07-19 fix batch.
Pinned by
`tests/security_hardening.rs::cve_floor_cargo_lock_contains_known_bad_versions`.

## Severity

**High** — YAML deserialisation runs on every DSL load, every
guard load, every trigger source load, and every operator config
load. Any parse-time UB is on us.

## Problem

`Cargo.toml:17` pins `serde_yml = "0.0.12"`. `cargo audit` (as of
2026-07-20) flags:

- **RUSTSEC-2025-0068** — `serde_yml` crate is unsound and
  unmaintained. Maintainer publicly abandoned the crate; no fix
  path upstream.
- **RUSTSEC-2025-0067** — transitive `libyml 0.0.5` (pulled by
  `serde_yml`) is unsound and unmaintained.

Full audit output preserved at
`projects/Ruuter-on-Rust/research/attacks/cargo-audit.txt`.

## Fix

Swap to `serde_yaml_ng`, a community-maintained fork of dtolnay's
original `serde_yaml`. Same public API `serde_yml` was aiming for,
so migration is mostly a Cargo.toml edit plus a `use` rename.

```toml
# Cargo.toml
- serde_yml = "0.0.12"
+ serde_yaml_ng = "0.10"
```

Then in every `.rs` file:

```rust
- use serde_yml::...
+ use serde_yaml_ng::...
```

Grep `src/` for `serde_yml` — fewer than 10 sites (DSL parser,
loader, config loader). Function signatures for
`from_str` / `from_slice` / `to_string` are identical.

## Acceptance

- `Cargo.toml` no longer references `serde_yml`.
- All `src/` code compiles against `serde_yaml_ng`.
- `cargo audit` no longer reports RUSTSEC-2025-0068 or
  RUSTSEC-2025-0067 (both fall off with the swap).
- Full test suite passes (`cargo test`).
- **Key** — `DSL-tests/` corpus still loads end-to-end. The DSL
  loader tests are the load-bearing regression signal for this
  swap; any parser subtlety difference between `serde_yml` and
  `serde_yaml_ng` will surface there.
- Remove the `("serde_yml", "0.0.12", ...)` and
  `("libyml", "0.0.5", ...)` rows from the `KNOWN_BAD` table in
  `tests/security_hardening.rs::cve_floor_cargo_lock_contains_known_bad_versions`.

## Non-goals

- Fully audit YAML anchor-bomb handling in the new crate — the
  existing test `dsl_yaml_anchor_bomb_does_not_hang_loader` is
  enough for the "bounded" contract. If `serde_yaml_ng` has
  tighter alias limits, note in CHANGELOG.

## Risk

Some `serde_yml`-only quirks (custom tags, `Value::from_iter`
edges) may need small adaptations. Fallback: `serde_yaml` (the
original, still readable but unmaintained) is API-compatible; but
choosing it re-opens a different advisory. Stay on `serde_yaml_ng`.

## Cross-reference

- `projects/Ruuter-on-Rust/REVIEW.md § S8`
- `projects/Ruuter-on-Rust/REMEDIATION.md § S8a`
- Related: task 055 (fast-float via boa upgrade), task 056
  (`cargo audit` CI gate). All three ship as one supply-chain
  cleanup PR per POST-FIX-REVIEW's recommendation.

Effort estimate: 30 min for the swap, plus DSL-tests full run.

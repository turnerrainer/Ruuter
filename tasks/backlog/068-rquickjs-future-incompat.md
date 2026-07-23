# 068 — Address rquickjs-core 0.6.2 future-incompat warning

## Filed

2026-07-24 — surfaced by the v0.7.0 release audit sweep. When
building with the `scripting-quickjs` feature, rustc emits a
`future-incompat` notice:

```
warning: the following packages contain code that will be rejected
by a future version of Rust: rquickjs-core v0.6.2
```

## Severity

**Low** — no current failure. `cargo clippy --features
scripting-quickjs --all-targets -- -D warnings` still passes on
stable rustc. The warning is a heads-up from rustc that specific
lints in `rquickjs-core 0.6.2` will graduate to errors in a future
release.

## Investigation

Run `cargo report future-incompatibilities --id 1` to see the exact
diagnostics — the id is emitted alongside the warning. Then:

- If an upstream rquickjs release has already addressed the lints,
  bump the dependency in `Cargo.toml` and re-run the full audit
  (`cargo clippy` on both feature sets, `cargo test`,
  `cargo audit --deny warnings`, `dsl-test`).
- If not, file upstream at the rquickjs repo and note the tracking
  issue link here.

## Non-goals

- Removing the `scripting-quickjs` feature (Boa remains the default;
  QuickJS is opt-in for consumers who want the perf profile).
- Silencing the warning without fixing it.

## Cross-reference

- `Cargo.toml` `[features]` block, `scripting-quickjs`.
- v0.7.0 CHANGELOG entry for the release audit sweep.

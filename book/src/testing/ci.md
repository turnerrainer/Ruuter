# CI integration

The shipped GitHub Actions workflow at `.github/workflows/tests.yml`:

```yaml
name: tests

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: cargo-${{ hashFiles('Cargo.lock') }}
          restore-keys: cargo-
      - name: Build
        run: cargo build --bin ruuter-on-rust --bin dsl-lint --bin dsl-test
      - name: dsl-lint (errors fail the build)
        run: ./target/debug/dsl-lint --dsl DSL --constants constants.ini
      - name: cargo test (existing Rust integration tests)
        run: cargo test --all
      - name: dsl-test (all .test.yml scenarios)
        run: ./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
```

## Stage ordering rationale

1. **Build** — compile the server + both test binaries. Cached across CI runs via `Cargo.lock` hash.
2. **`dsl-lint`** — catches broken DSLs in ~100 ms before any harness spins. If a rename removed a `next:` target, this fails first with a clear message.
3. **`cargo test`** — runs `tests/*.rs` integration tests: engine invariants, guards, state store, ws server, ws source, iterate, supervisor, trigger dispatcher. These are the framework's contract tests.
4. **`dsl-test`** — runs every `.test.yml` scenario against the full framework stack. Slowest stage (~2 s on the sample corpus). Runs last so an obvious DSL bug fails in stage 2 or 3 first.

Fail-fast is intentional: `dsl-lint` catching a typo is a better developer experience than `dsl-test` catching the same typo after building a router.

## Local pre-push

```bash
cargo build --bin dsl-lint --bin dsl-test \
  && ./target/debug/dsl-lint --dsl DSL --constants constants.ini \
  && cargo test --all \
  && ./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
```

Roughly 10 s on a warm build, ~40 s on a cold build.

## Pre-commit hook

Only run `dsl-lint` at commit time — the other stages take too long for interactive use.

`.git/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
set -e
cargo build --bin dsl-lint --quiet
./target/debug/dsl-lint --dsl DSL --constants constants.ini
```

## Machine-readable output for gate rules

Both binaries emit JSON with `--json`:

```bash
./target/debug/dsl-lint --json | jq '.errors, .warnings, .files_scanned'
./target/debug/dsl-test --json | jq '.passed, .failed, .total'
```

Use this for external gating (e.g. "block PR if warnings increase from main").

## Extending the workflow

- **Add a matrix on rust-toolchain versions**: verify DSL semantics stay stable across the versions you support in production.
- **Publish `--json` outputs as artifacts**: keeps history of pass/fail counts over time.
- **Add a `dsl-lint --include-disabled` job**: catches shape regressions in `.yml.disabled` sample files (WS sources) that aren't loaded at boot but are exercised only when an operator renames them.

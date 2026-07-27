# Testing

Two binaries ship with Ruuter-on-Rust for validating DSL trees end-to-end:

- **`dsl-lint`** — static validator. Parses every DSL, checks step-graph integrity, resolves `[#constant]` references, verifies template targets. No execution. Runs in ~100 ms on the sample corpus.
- **`dsl-test`** — runtime test runner. Walks `DSL-tests/`, executes scenarios against the full framework stack (CSRF, traceparent, guards all active), asserts on HTTP response, WebSocket frames, state-store contents, and mock upstream calls.

Both binaries live under `src/bin/` and are built alongside the server:

```bash
cargo build --bin ruuter-on-rust --bin dsl-lint --bin dsl-test
```

## Layout

Tests live in a `DSL-tests/` tree mirroring `DSL/`:

```
DSL/
  samples/
    GET/basic/hello.yml          ← the DSL
DSL-tests/
  samples/
    GET/basic/hello.test.yml     ← its tests
  framework/                     ← invariants that aren't tied to a DSL
    fallback-404.test.yml
    traceparent.test.yml
```

Rationale: separate trees keep `DSL/` clean (production surface), tests parallel the DSL they cover (obvious ownership), and framework-invariant tests get their own top-level directory since they don't belong to any single DSL.

## What each layer catches

| Layer | Tool | Cost | Catches |
|---|---|---|---|
| Static | `dsl-lint` | ~100 ms | Parse errors, unresolved `next:` refs, missing constants, broken template targets, unreachable steps, missing source `kind:` fields |
| Rust unit / integration | `cargo test` | seconds | Engine invariants (state store isolation, guard prefix matching, ws source dispatch, iterate bounds, supervisor restart) |
| Scenario | `dsl-test` | seconds | Per-DSL contract: HTTP status, response body, headers, state mutations, mock-upstream calls, WS frame exchange |

## Verified corpus

Build both binaries:

```bash
cargo build --bin dsl-lint --bin dsl-test
```

Static lint:

```bash
./target/debug/dsl-lint --dsl DSL --constants constants.ini
```

```
dsl-lint: 61 file(s) scanned, 61 ok, 0 error(s), 3 warning(s)
```

Scenario runner:

```bash
./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
```

```
dsl-test: 99 scenario(s) — 99 passed, 0 failed
```

The three warnings are unresolved constant references in DSLs that
document external integration points (`API_KEY`,
`aapl_alert_webhook`, `stock_alert_webhook`) — they're expected to
be provided by the operator, not by the shipped `constants.ini`.
Both `[#KEY]` and `#{KEY}` forms are recognised by the linter.

## Design principles

- **Data-driven, not code.** Test files are YAML with a small assertion vocabulary. Adding coverage for a new DSL is one file, zero Rust.
- **Full stack, not mocked guts.** Scenarios flow through `DslRouter::build_axum_router` via `tower::ServiceExt::oneshot` — the same code path a real HTTP request takes. Framework middleware (CSRF, traceparent, method allow-list) runs unmodified.
- **Hermetic per file.** Each `.test.yml` gets a fresh `DslLoader` load with its own constant overrides. Files don't leak state into each other.
- **In-file scenarios share state.** Within one file, scenarios run in declaration order against one `Harness`. This lets you write "first call sets counter=1, second call returns counter=2" style tests without seeding state twice.
- **Test what's shipped, not what you wish were shipped.** If a DSL sample has a bug, the test documents the current behaviour and CI turns green. Fix the DSL, flip the assertion.

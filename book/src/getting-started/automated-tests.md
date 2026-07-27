# Watch the automated tests pass

Ruuter ships two independent test suites. Both are meant to be run on
every code or DSL change; both are meant to finish in seconds.

You need the [Rust toolchain](./prerequisites.md) for this chapter.
Skip to [Postman](./postman.md) if you only want to exercise a running
server.

## Build

```bash
cargo build --bin ruuter-on-rust --bin dsl-lint --bin dsl-test
```

First build is a few minutes; incremental builds are seconds.

## 1. Rust unit + integration tests

Framework invariants (state store isolation, guard matching, SSRF,
supervisor restart, traceparent, ...):

```bash
cargo test --no-fail-fast
```

Expected on the 0.8.0-rc.1 baseline:

```
test result: ok. 222 passed; 0 failed; 3 ignored
```

The 3 ignored tests are timing-sensitive scenarios kept out of the
default run to avoid CI flakes.

## 2. DSL linter — static validation

Parses every DSL, verifies step-graph integrity, resolves `[#constant]`
references, checks template targets. Does not execute anything:

```bash
./target/debug/dsl-lint --dsl DSL --constants constants.ini
# dsl-lint: 62 file(s) scanned, 62 ok, 0 error(s), 3 warning(s)
```

The 3 warnings are constant references pointing to operator-provided
values (`API_KEY`, `aapl_alert_webhook`, `stock_alert_webhook`); they're
expected, not defects. Both `[#KEY]` and `#{KEY}` forms are recognised.

## 3. DSL scenario runner — end-to-end DSL tests

Executes every `DSL-tests/**/*.test.yml` scenario against the real
framework stack (CSRF, traceparent, guards, method allow-list, ...).
Assertions cover HTTP response, state mutations, WebSocket frames, and
mock upstream calls:

```bash
./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
# dsl-test: 100 scenario(s) — 100 passed, 0 failed
```

## Why three separate suites

| Suite | Catches | Runs in |
|---|---|---|
| `cargo test` | Engine bugs (Rust code paths) | ~seconds |
| `dsl-lint` | DSL typos / broken refs / missing constants | ~100 ms |
| `dsl-test` | Contract of shipped DSLs (status, body, state, mocks) | ~2 s |

Every layer is meant to fail early on a different class of mistake.

Next: [Try the Postman collection](./postman.md).

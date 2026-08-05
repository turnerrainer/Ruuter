# Expected baseline for `compat/` parse gate

Snapshot: 2026-08-04. Rust build: `audit/fixes-2026-08-04` HEAD.

## Errors

**Zero.** Any error is a build failure.

## Warnings

Three, all `unreachable step` warnings on Java demos that
deliberately construct DSLs where some steps are never reached from
the entry point (they exist to demonstrate `next:` jump semantics).

| File | Step | Rationale |
|---|---|---|
| `compat/java-ruuter/DSL/GET/common/skip.yml` | `second_step` | Java's `skip:` demo: `second_step` is `skip: true`, so control flows over it. Rust's reachability analysis flags it. |
| `compat/java-ruuter/DSL/GET/order/end-execution.yml` | `this_step_is_not_executed` | Java's `end` sentinel demo: the entry step ends execution before this one. |
| `compat/java-ruuter/DSL/GET/order/jump-over-step.yml` | `this_step_is_jumped_over_and_not_executed` | Java's `next:` jump demo: entry step jumps past this one. |

These warnings are **expected**. The CI gate does not fail on them.
If any new warning appears (or one of these three disappears), the
maintainer should investigate — a new warning may be a new
divergence to file.

## Regenerating

```sh
cargo build --release --bin dsl-lint
./target/release/dsl-lint --dsl compat/java-ruuter/DSL --constants constants.ini | tee compat/last-run.log
./target/release/dsl-lint --dsl compat/java-ruuter/samples --constants constants.ini | tee -a compat/last-run.log
```

Compare against the tables above.

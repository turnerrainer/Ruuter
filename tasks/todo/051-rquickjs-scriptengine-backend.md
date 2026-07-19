# 051 — Adopt rquickjs as an alternative ScriptEngine backend (unblocks 036 + 045)

## Filed

2026-07-19 — direct follow-up to the task 047 spike. Spike answered
the gating question: `rquickjs` with `parallel + futures` features
provides `Send + Sync` Runtime and Context types. This forecloses
the "wait for Boa 0.20+" and "build a dedicated OS worker pool"
paths — a QuickJS backend cleanly unblocks tasks 036 and 045 that
were blocked on Boa's `!Send` types.

## Problem

Tasks 036 (per-request BoaContext pool) and 045 (pre-parsed Script
cache) are the two biggest Boa-perf wins in the roadmap. Both
require holding a JS context on `ExecutionContext` across `.await`
boundaries. `boa_engine::Context` embeds `Rc<...>` internals and
is `!Send + !Sync`. No amount of `Arc<Mutex<...>>` wrapping fixes
this — the framework's async architecture requires `Send` at
every `.await`.

The spike (task 047, done) confirmed rquickjs solves this. Now the
work is: build a rquickjs-backed ScriptEngine, ship it behind a
feature flag as `scripting-quickjs`, and use its Send-compatibility
to actually implement 036 and 045.

## Fix

Introduce a `ScriptEngine` trait; implement it twice:

```toml
[features]
default = ["scripting-boa"]
scripting-boa = ["boa_engine"]
scripting-quickjs = ["rquickjs"]
# mutually exclusive
```

```rust
trait ScriptEngine: Send + Sync {
    fn evaluate(&self, input: &Value, ctx: &ExecutionContext) -> Result<Value>;
    fn evaluate_tracked(&self, input: &Value, ctx: &ExecutionContext) -> Result<(Value, bool)>;
}

#[cfg(feature = "scripting-boa")]
pub struct BoaScriptEngine { ... }  // current impl, renamed

#[cfg(feature = "scripting-quickjs")]
pub struct QuickJsScriptEngine {
    context: Arc<rquickjs::AsyncContext>,
    // ... limits, cache
}
```

Runtime-limit mapping:
- `max_loop_iterations` → rquickjs `Runtime::set_max_stack_size` +
  interrupt handler set via `Runtime::set_interrupt_handler`
- `max_stack_size` → same interrupt handler pattern

Once the QuickJS backend exists AND is Send:
- Task 036: add `Arc<AsyncContext>` field to `ExecutionContext`,
  reuse across steps in one request.
- Task 045: pre-parse expressions at DSL load, cache
  `rquickjs::persistent::Persistent<Function>` (or similar) in a
  shared map, invoke at request time.

## Acceptance

- New `src/scripting/quickjs.rs` implementing `ScriptEngine` trait
  under the `scripting-quickjs` feature.
- All 15 existing tests in `tests/scripting_037_literal_fastpath.rs`
  pass on the QuickJS backend (same fast-path semantics preserved).
- Every `.test.yml` scenario in `DSL-tests/` passes byte-identical
  outputs on both engines. This is the ECMAScript-compatibility
  gate — if QuickJS diverges from Boa on our corpus, the divergence
  is a shipping blocker until resolved.
- Bench: extend `bench/run-ab-comparison.sh` with a boa-vs-quickjs
  pair. Both engines on the same DSLs (`js-heavy`, `path-params`,
  `guarded`) — report deltas.
- CI matrix: build + test both `--features scripting-boa` and
  `--features scripting-quickjs`. Same test suite, same DSLs.
- Docs: `book/src/framework/scripting-engines.md` (new) — how to
  pick which engine, what the compatibility guarantees are, what
  to do if a DSL behaves differently.

## Then 036 + 045 unblock

Once 051 lands with QuickJS Send-compatible:

- Task 036 becomes a small change: add `Arc<AsyncContext>` to
  `ExecutionContext`, thread it through `evaluate()` calls, drop
  the per-call context construction. Expected 3-5× on Boa-hitting
  DSLs by amortising context setup.
- Task 045 becomes tractable: pre-parse each `${...}` at DSL load,
  cache the parsed callable, invoke at runtime. Expected additional
  1.5-2×.

Combined (036 + 045 + 051) expected impact: **5-10× on the
js-heavy / guarded / path-params scenarios**, all of which are
currently ~1-3k rps. Would put DSL-heavy throughput in the same
ballpark as the framework baseline (~50-100k rps), effectively
eliminating Boa/QuickJS as the perf ceiling.

## Non-goals

- Making QuickJS the default. Boa stays default until QuickJS
  proves out on the full corpus + a release cycle of real-world use.
- V8/SpiderMonkey/JavaScriptCore. Same rejection reasons as before
  (binary size, init cost, C++ CVE surface).
- Sharing engines across projects. Per-project pool only, same as
  the original 036 scope.

## Risk

- **ECMAScript compatibility drift.** Byte-identical outputs on the
  DSL-tests corpus is the gate. If QuickJS parses a Date literal
  differently or gives a different `Number.prototype.toString`
  precision at boundaries, DSL authors would see behaviour changes
  when switching engines. Mitigation: keep the mutually-exclusive
  feature gate, ship as opt-in, document all known deltas.
- **CVE surface.** QuickJS is a C library. The Rust wrapper
  (`rquickjs`) audits soundness of the FFI boundary; QuickJS
  upstream has occasional CVEs (nothing severe historically, but
  non-zero). Ops story: subscribe to QuickJS advisories, budget
  for occasional dep bumps.
- **Feature-flag CI matrix.** Doubles the test surface. Manageable
  (both flavours run the same tests + DSL scenarios); not free.
- **Binary size.** +500 KB for rquickjs + rquickjs-sys. Small
  compared to the perf gain but worth calling out for
  size-constrained deployments.

## Sequencing

051 first (this task) → then reopen 036 (per-request context pool)
→ then reopen 045 (pre-parsed cache). 051 is the enabler; 036 and
045 become small once it lands.

# 012 — Expose `state.get/set/delete` as Boa JS bindings

**Status**: BLOCKED on #017 (externalize StateStore).

**Why blocked**: surfacing the state API further before its backing
implementation is externalized would cement an interface that the
externalization may want to revise (e.g. async signatures if the
backend becomes Redis/Resql). Re-evaluate once #017 lands.
**Severity**: LOW (today the step-type interface — read → JS → write —
covers the use case; this is ergonomics for compact DSLs).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.
**Follow-up to**: #003.

## What's wrong (mildly)

After #003, DSLs use a three-step pattern to read-then-modify state:

```yaml
read:   { state: { get: { key: "x", into: x } }, next: bump }
bump:   { assign: { y: "${x + 1}" }, next: write }
write:  { state: { set: { key: "x", value: "${y}" } } }
```

For DSLs that want concise expressions, exposing `state.get(key)` /
`state.set(key, value)` inside `${...}` would collapse this:

```yaml
inc:
  assign:
    counter: "${state.set('x', (state.get('x') ?? 0) + 1)}"
```

## Fix

In `src/scripting/mod.rs::setup_bindings`, register a global `state`
object on the Boa context with three native functions backed by the
captured `(project, StateStore)` pair:

```rust
boa.register_global_callable(
    JsString::from("__state_get"), 1,
    NativeFunction::from_copy_closure_with_captures(
        move |_, args, _, captures| { /* serialize captures.0.get(...) */ },
        (state_clone, project_clone),
    ),
);
// Then evaluate a small JS prelude:
//   var state = { get: __state_get, set: __state_set, delete: __state_delete };
```

`NativeFunction` requires the closure to be `'static + Send + Sync`,
which the `Arc<DashMap>` inside `StateStore` already supports.

## Verification

- Unit test: a single-step DSL that reads + writes state via JS
  bindings returns the expected value after N requests.
- Sample `DSL/samples/POST/state/inc-concise.yml` mirroring the
  multi-step `inc.yml` produces identical results.

## Why this is generic

Pure scripting-engine ergonomics. No service knowledge.

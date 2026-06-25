# 011 — DSL execution starts at a random step (HashMap iteration order)

**Status**: BACKLOG.
**Severity**: HIGH (correctness; ~50% of multi-step DSLs fail intermittently).
**Effort**: 1-2 hours.
**Filed**: 2026-06-17.
**Discovered while**: smoke-testing `/samples/javascript/array-operations`
after the #002 Boa-reuse refactor — got
`ReferenceError: numbers is not defined`. Root cause is pre-existing,
not from the refactor.

## What's wrong

`src/dsl/mod.rs` stores steps as `HashMap<String, DslStep>` and
`step_names()` returns `self.steps.keys().cloned().collect()` — order
is nondeterministic. The executor (`src/router/mod.rs:73-104`) then
treats `step_names[0]` as the entry point.

The Ruuter convention (inherited from the Java implementation) is
that **the first step in YAML source order is the entry point**.
Today that holds only by luck.

## Reproducer

`DSL/samples/GET/javascript/array-operations.yml` has three steps:
`create_array` → `process_array` → `respond`. After restarting the
server, the request fails or succeeds depending on which step is
first in the per-process HashMap iteration:

```
$ curl http://localhost:8080/samples/javascript/array-operations
{"error":"Script evaluation error: ReferenceError: numbers is not defined"}
```

`math-operations` and `date-time` happen to work — same root issue,
different luck.

## Fix

Switch `Dsl.steps` to an order-preserving map. Two paths:

1. **`indexmap::IndexMap<String, DslStep>`** — drop-in, deserialises
   in source order via `serde_yaml`. Add `indexmap` to `Cargo.toml`.
2. **`Vec<(String, DslStep)>`** + lookup index — explicit but more
   churn.

Option 1 is the right answer. `step_names()` then iterates the
IndexMap in insertion order.

Also: `parse_steps` in `src/dsl/parser.rs` currently takes
`HashMap<String, YamlValue>` from `serde_yaml::from_str` — change to
`indexmap::IndexMap<String, YamlValue>` so source order survives the
parse, then insert into the new ordered `Dsl.steps`.

## Verification

- `/samples/javascript/array-operations` returns the populated array
  after every restart (run 10 times, expect 10 successes).
- All existing sample DSLs continue to return the same output they do
  on a "lucky" run today.

## Why this is generic

Pure DSL-engine correctness. No service knowledge required.

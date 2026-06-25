# 003 — Shared mutable state context (project-scoped key/value)

**Status**: DONE 2026-06-17.

## Scope boundary (added post-completion)

`StateStore` is **ephemeral, process-local state only**. Intended use:

- Rolling windows / ring buffers for signal processing
- Hot counters, rate-limit buckets, dedup sets
- Indicator caches that can be rebuilt from upstream sources
- Idempotency markers that don't need to survive a restart

For **persistent state** (positions, orders, audit records, anything
that must survive a Ruuter restart or be queried by other components
in the stack), use **Resql** instead — write a `.sql` file, expose it
as a REST endpoint, and call it from a DSL `http` step. Persistent
storage is Resql's dedicated role in the Buerostack ecosystem; Ruuter's
in-memory store is not a database.

(This boundary mirrors the same principle that ruled out a cron source
in Ruuter — see #006-CANCELLED. CronManager owns scheduling, Resql
owns persistence, Ruuter owns routing + event-trigger orchestration +
ephemeral state.)

---

## Original task body
**Severity**: HIGH (prerequisite for any DSL that must remember
anything across requests — rolling buffers, counters, dedup sets,
positions, indicators, idempotency keys).
**Effort**: 1 day.
**Filed**: 2026-06-17.
**Blocks**: #005, #006.

## What's wrong

`ExecutionContext` is constructed fresh per request
(`src/router/mod.rs:64`). There is no way for one DSL execution to
observe state written by an earlier execution. Every event-driven or
stateful use case (whether trading, IoT, chat, counters, anything) is
blocked on this.

## Fix

Add a process-wide store:

```rust
pub struct StateStore {
    inner: DashMap<StateKey, serde_json::Value>,
}

#[derive(Hash, Eq, PartialEq)]
pub struct StateKey {
    pub project: String,
    pub key: String,
}
```

Wire it via:

1. New step type `StateStep { state: StateOp }` where `StateOp` is one
   of `{ get: String, into: String }` / `{ set: { key: String, value: Value } }` /
   `{ delete: String }`.
2. JS bindings in `ScriptEngine::setup_bindings`: expose a `state`
   object with `get(key)`, `set(key, value)`, `delete(key)` backed by
   sync `DashMap` operations.
3. Keys are namespaced by project (the first path segment in
   `handle_request`). DSLs cannot read across projects.

`StateStore` is constructed in `main.rs` and shared by both the HTTP
router and (later) the WebSocket source via `Arc`.

## Verification

- DSL test: a `POST /counter/inc` DSL increments and returns a value
  that survives across requests.
- DSL test: project isolation — `/a/state` cannot read `/b/state`'s
  keys.
- Concurrency: hammer two endpoints incrementing the same key from
  100 parallel requests; final count is exact.

## Why this is generic

The store is opaque `serde_json::Value`. Ruuter has no notion of what
the values mean — positions, counters, cached responses, dedup
markers, all live under the same generic primitive.

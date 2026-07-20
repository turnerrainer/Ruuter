# 042 — Single-flight positive-response coalescing

## Filed

2026-07-15 — complements task 038 (Cuckoo dedup for negative-cache).
Any DSL whose expensive path (DB join + fan-out + assembly) can be
triggered by concurrent requests keyed on the same input needs a
coalescing primitive; the framework currently offers none.

## Problem

Task 038's Cuckoo filter handles set-membership ("have I seen X?").
It does NOT handle "two concurrent requests want the same expensive
computation — only run it once and share the answer."

Concrete pattern: N clients ask the same expensive question within
the same short window. Today each triggers an independent execution
of the DSL — N database joins, N fan-outs, N response builds.
Single-flight collapses them into 1 execution + (N-1) wait-and-share.

## Fix

New step primitive — `single_flight`:

```yaml
lookup:
  single_flight:
    key: "uil-query:${incoming.body.uil}:${incoming.query.subset}"
    ttl_ms: 1000                     # coalesce window
    do:
      - parallel_http:
          peers: "${gates}"
          args: {...}
          aggregate: first_n
          first_n: 1
          result: match
      - assign: { answer: "${match}" }
    result: answer
  next: reply
```

Semantics:

- First caller with a given `key` runs `do:` and becomes the "leader".
- Concurrent callers with the same `key` block on the leader's result
  (waiting via a shared `tokio::sync::broadcast` channel per key).
- After `ttl_ms` expires (or the leader completes), the entry is
  removed and the next caller becomes a fresh leader.
- Result binding is identical whether you were leader or follower.
- Leader failure propagates to followers (they see the same error).

Storage: `DashMap<String, Arc<Mutex<Option<Result>>>>` with a broadcast
channel per entry. Bounded via a max-key count; oldest evicted on
overflow.

## Interaction with the DSL-authored idempotency pattern

Overlaps but distinct:

| Primitive | Purpose | Scope | Persistence |
|---|---|---|---|
| DSL idempotency pattern (`state.get`/`state.set` with `origin + endpoint + body-hash` key) | Retry safety across the wire | Per-DSL identity+body dedup key | Whatever the state backend offers (TTL, shared) |
| single_flight (this task) | Coalesce concurrent duplicate work | Per-DSL-computed key | In-flight only, no cache after completion |
| Cuckoo (task 038) | "Have I seen this before?" | Set membership | In-memory bloom-like |

They compose: a DSL that dedups on `state.get(idempotency-key)` can
call `single_flight` inside the `work:` branch to coalesce a
concurrent burst before it hits the backing store; the Cuckoo filter
can gate the DSL entirely.

(Framework-level `Idempotency-Key` handling was removed in v1.0.0 —
see `book/src/dsl/idempotency-pattern.md` for the shape.)

## Acceptance

- New `single_flight` step with `key`, `ttl_ms`, `do`, `result`.
- Nested steps in `do:` execute in the leader's context; followers
  wait and receive the same output.
- Test: 100 concurrent requests to the same URL with identical body →
  verify `do:` body executed exactly once (via a state counter).
- Test: leader panics → followers see the panic as a step error.
- Documented in `book/src/dsl/steps/single_flight.md`.

## Non-goal

Cross-instance coalescing. Two Ruuter replicas each maintain their
own single-flight map — a duplicate on the other replica is not
coalesced. Solving that requires a shared state backend that the DSL
addresses via `state.get`/`state.set`, not a framework-level cache.

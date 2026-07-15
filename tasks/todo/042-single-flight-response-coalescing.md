# 042 — Single-flight positive-response coalescing

## Filed

2026-07-15 — complements task 038 (Cuckoo dedup for negative-cache).
Addresses eFTI Gate burst weakness B9 in
`h2ck.me/projects/efti-gate-dlk/SCALABILITY.md`:

> Two cameras 5 km apart photograph the same truck within seconds;
> the authority's app fires both. The gate handles each independently:
> same DB join, same fan-out to all peers, same response build. There
> is no deduplication layer.

## Problem

Task 038's Cuckoo filter handles set-membership ("have I seen X?").
It does NOT handle "two concurrent requests want the same expensive
computation — only run it once and share the answer."

For eFTI: 27 authority app-points asking about the same UIL in the
same 500 ms window today issue 27 independent broadcasts. Single-flight
collapses them into 1 broadcast + 26 wait-and-share.

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

## Interaction with Idempotency-Key

Overlaps but distinct:

| Primitive | Purpose | Scope | Persistence |
|---|---|---|---|
| Idempotency-Key (framework) | Retry safety across the wire | Per-client-declared key | TTL cache, single-instance |
| single_flight (this task) | Coalesce concurrent duplicate work | Per-DSL-computed key | In-flight only, no cache after completion |
| Cuckoo (task 038) | "Have I seen this before?" | Set membership | In-memory bloom-like |

They compose: an `Idempotency-Key`-guarded route can use `single_flight`
inside its DSL to coalesce; the Cuckoo filter can gate the DSL
entirely.

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
coalesced. Solving that requires the same shared-store work as task
029 (Idempotency shared store); track separately.

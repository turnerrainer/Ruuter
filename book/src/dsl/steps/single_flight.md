# single_flight

Collapse concurrent duplicate requests into one execution + N wait-and-share followers.

```yaml
lookup:
  single_flight:
    key: "cache-warmer:${incoming.body.uil}"    # DSL-computed coalesce key
    ttl_ms: 1000                                # follower wait budget
    do:
      - http:
          call: http.get
          args: { url: "https://slow-upstream/$_{'${incoming.body.uil}'}" }
          result: upstream
      - assign:
          answer: "${upstream.body}"
    result: answer                              # variable to snapshot + share
  next: reply
```

## Semantics

- **First arrival with a given `key` is the leader.** It runs `do:` in its own context; when finished it broadcasts the value of the variable named by `result:` to any waiting followers, then removes the key from the registry.
- **Concurrent arrivals with the same `key` are followers.** They subscribe to the leader's outcome and block until the leader publishes.
- **Sequential calls do NOT coalesce.** Once the leader completes and removes the entry, the next arrival is a fresh leader.
- **`ttl_ms` bounds follower waiting, not leader execution.** If the leader hasn't published within `ttl_ms`, each follower returns a Timeout error and the slot is evicted so the *next* caller becomes a fresh leader. The current leader is not interrupted — its own step budgets apply.
- **Leader errors propagate.** If `do:` fails, every follower sees an error containing the leader's failure message rather than silently succeeding with a stale value.
- **`result:` is optional.** When set, the leader's post-body variable of that name is snapshotted and bound into every follower's context under the same name. When unset, followers still complete once the leader does, but no variable is bound (useful for cache-warming DSLs where the return value comes from a subsequent step).

## When to use

- Two clients ask for the same expensive computation within a short window. Without coalescing, both trigger the full pipeline; with coalescing, the second waits on the first.
- Cache-warming patterns: the first request pays; every concurrent duplicate rides for free.
- Fan-out queries where the underlying data changes slowly relative to the request rate.

## When NOT to use

- **Per-user state that must be fully isolated.** If two users happen to key on the same string, they'll share the leader's response — that's the whole point, but for privacy-sensitive DSLs, `key:` must include a per-user discriminator (`"lookup:${incoming.headers['x-user-id']}:${...}"`).
- **Low-cost operations.** The registry overhead is not free; for a DSL that takes 100µs, coalescing costs more than it saves.
- **Multi-replica dedup.** The registry is process-local. Two Ruuter pods each maintain their own map. See the *Non-goals* section.

## Composition

- Composes with the **[DSL idempotency pattern](../idempotency-pattern.md)**: a route that dedups on `state.get(idempotency-key)` can wrap the `work:` branch in `single_flight` for concurrent-duplicate collapsing before the state store is hit.
- Composes with **iterate**: a `single_flight` step can appear inside `iterate.do`, though usually the reverse is more useful (many items, each guarded by its own single_flight window).
- Composes with **template**: the `do:` body can invoke another DSL via `template:` for reuse of computation-heavy work.

## Interaction with framework features

| Feature | Behaviour |
|---|---|
| Guards | Run per-request (before single_flight step is reached), not coalesced |
| CSRF Origin check | Same as guards |
| DSL idempotency pattern | DSL-level `state.get`/`state.set` runs inside the DSL body; single_flight coalesces the leader's `work:` branch |
| Traceparent | Leader's traceparent is the one carried into `do:`; followers keep their own traceparent for their own response |
| max_step_recursions | Applies to leader's transitions inside `do:` normally |
| Response headers | Each follower builds its own response headers via subsequent DSL steps; leader's headers do NOT propagate |

## Non-goals

- **Cross-instance coalescing.** Two Ruuter pods each maintain their own single_flight map — a duplicate landing on the other pod is not coalesced. Solving that requires a shared state backend that the DSL addresses via `state.get`/`state.set` (see the [DSL idempotency pattern](../idempotency-pattern.md)), not a framework-level cache.
- **LRU eviction.** The registry has a soft cap (default 10 000 distinct in-flight keys); on overflow an arbitrary entry is evicted. LRU would need extra bookkeeping and isn't v1 scope. In practice the cap only engages under a pathological DSL that keys on unbounded distinct values (per-request UUIDs).
- **Value caching after the leader completes.** `single_flight` is in-flight coalescing only — the moment the leader publishes, the slot is removed. For time-bounded caching, wrap `single_flight` output in a `state.set` with a TTL.

## Common pitfalls

- **Key includes runtime timestamps.** `${Date.now()}` in the key produces a fresh string every request → nothing coalesces. Use inputs that are stable across the coalesce window.
- **Body has side effects on `incoming.*`.** The leader's `do:` mutates the leader's context; followers get only the `result:` variable snapshot, not the full context. If your DSL reads other leader-side variables downstream, they won't be there on followers.
- **`ttl_ms` too tight for the DSL's real latency.** Followers time out before the leader is done → the DSL fails under exactly the load it was supposed to help with. Set `ttl_ms` to a safe upper bound on your `do:` body's realistic p99 latency, plus headroom.

## Runnable example

`DSL/samples/POST/advanced/single-flight-lookup.yml` (elided):

```yaml
coalesce:
  single_flight:
    key: "sf-demo:${incoming.body.id}"
    ttl_ms: 3000
    do:
      # Simulate an expensive lookup — ~120 ms of busy work.
      - iterate:
          over: "${Array.from({length: 300}, (_, i) => i)}"
          as: n
          do: [ { assign: { sink: "${n * n}" } } ]
      # Bump the shared execution counter so we can prove coalescing.
      - state: { get: { key: "sf_demo_count", into: prior } }
      - assign: { exec_count: "${(prior == null ? 0 : prior) + 1}" }
      - state: { set: { key: "sf_demo_count", value: "${exec_count}" } }
      - assign:
          shared_result:
            id: "${incoming.body.id}"
            execution_count: "${exec_count}"
            computed_at: "${Date.now()}"
    result: shared_result
  next: respond

respond:
  return: "${shared_result}"
  next: end
```

Single request — one execution, `execution_count = 1`:

```console
$ curl -sX POST http://localhost:8080/samples/advanced/single-flight-lookup \
    -H 'Content-Type: application/json' -d '{"id":"item-42"}'
{"computed_at":1785079302100.0,"execution_count":1,"id":"item-42"}
```

Five concurrent requests on the **same id** — all five see the same
`computed_at` and `execution_count`, proving only one real execution
ran:

```console
$ for i in 1 2 3 4 5; do
    curl -sX POST http://localhost:8080/samples/advanced/single-flight-lookup \
        -H 'Content-Type: application/json' -d '{"id":"burst-1"}' &
  done; wait
{"computed_at":1785079305810.0,"execution_count":2,"id":"burst-1"}
{"computed_at":1785079305810.0,"execution_count":2,"id":"burst-1"}
{"computed_at":1785079305810.0,"execution_count":2,"id":"burst-1"}
{"computed_at":1785079305810.0,"execution_count":2,"id":"burst-1"}
{"computed_at":1785079305810.0,"execution_count":2,"id":"burst-1"}
```

(Timestamps and counter value differ per invocation; the important
thing is that all five parallel responses share the same
`computed_at` and `execution_count`.)

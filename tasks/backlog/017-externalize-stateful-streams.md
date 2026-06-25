# 017 — Externalize all stateful streams (statelessness compromise from #003)

**Status**: BACKLOG.
**Severity**: HIGH (architectural — violates a core Buerostack claim).
**Effort**: 2-3 days (design + implementation + migration of #003).
**Filed**: 2026-06-17.
**Architectural priority**: requires full attention before any service
that depends on event-stream state ships to production.
**Blocks**: #007 (ring-buffer step), #012 (state JS bindings).
**Created in response to**: Rainer's 2026-06-17 review noting that
the "stateless architecture" principle is core, not aspirational.

## What's wrong

`src/state/StateStore` (delivered as #003) lives in the Ruuter
process via a `DashMap`. This was a deliberate scoping decision at
the time (#003 done-note: "ephemeral process-local state only") but
it breaks Buerostack's foundational stateless-architecture claim in
two ways:

1. **Horizontal scaling is no longer safe.** Two Ruuter replicas
   behind a load balancer each maintain their own state. A counter
   incremented on replica A is invisible to replica B. The
   ARCHITECTURE.md §9 promise — "Stateless Architecture / Horizontal
   scaling for DDoS mitigation" — is materially false the moment a
   service uses `StateStore`.

2. **Restart loses everything.** A pod restart, OOM-kill, or rolling
   deploy wipes every rolling window, every dedup marker, every
   counter. For an event-driven service (the use case that motivated
   #003) this is a silent-correctness hazard, not a tolerable
   tradeoff.

The compromise also propagates: the WebSocket source (#005),
trigger dispatcher (#004), supervisor (#008), and proposed
ring-buffer step (#007) all assume `StateStore` is a reliable
substrate. They aren't, today.

## Fix — design space (decision required)

### Option A: Redis-backed StateStore

Replace `DashMap` with a Redis client behind the same `StateStore`
trait. Every Buerostack deployment gains a Redis dependency. Pros:
fast, well-understood, supports atomic primitives (INCR, ZADD for
sorted windows). Cons: new operational component; Redis is not yet
a Buerostack-blessed component.

### Option B: Resql-backed StateStore

State goes into a PostgreSQL schema (one per project, in line with
existing multi-tenant convention) accessed via Resql `.sql` files.
Pros: stays inside the existing component family; persists across
restarts; auditable via append-only convention. Cons: latency floor
of a network round-trip per get/set; conflicts with append-only
(state mutates by definition) — needs a "current-value view over
append-only writes" pattern.

### Option C: Externalize the event stream itself

Source tasks (WebSocket consumers) publish to a real broker
(Kafka/NATS/Redis-Streams) instead of dispatching in-process. Each
service consumes from the broker as a separate horizontally-scaled
worker, with offsets persisted by the broker. State is reconstructed
from the stream on startup. Pros: textbook stateless event-driven
architecture; survives any single-node failure. Cons: substantial
re-architecture; introduces a new core dependency; #005's in-process
dispatch path becomes obsolete.

### Option D: Hybrid

Keep `StateStore` as a write-through cache to Redis/Resql. Ruuter
process can serve hot reads from memory but every write is durably
externalized. Pros: latency benefit of in-process + correctness of
external. Cons: cache-invalidation problem across replicas (the same
problem Option A solves outright).

**Recommendation pending Rainer's review.** Option C is the most
architecturally pure but biggest lift. Option B aligns with the
existing component model. Option A is fastest to implement but adds
a non-Buerostack dependency.

## Migration of #003

Whichever option lands, the existing `state` step type and its YAML
surface (`{ get | set | delete }`) should remain unchanged so DSL
authors see no break. Only the backing implementation moves.

## In the meantime

Until #017 is resolved:

- `StateStore` SHOULD be treated as best-effort ephemeral cache.
- The 2026-06-17 done-note on #003 stands: persistent state belongs
  in Resql, period.
- New work that would *increase* reliance on `StateStore` (e.g.
  #007 ring-buffer step) is BLOCKED on this task.
- ARCHITECTURE.md §9 should be amended to reflect the current
  reality: stateless gateway, **opt-in stateful event-driven
  components requiring external backing once that backing exists**.

## Verification (post-implementation)

- Two-replica deployment: write counter via replica A, read via
  replica B, observe consistent value.
- Restart-kill replica A mid-stream: counter value survives,
  rolling-window reconstruction completes within X seconds of
  restart.
- Load test: 1000 req/s on `state.set` from each of 4 replicas;
  final count matches expected (no lost updates under contention).

## Why this is generic

The state-externalization problem is service-agnostic. Whatever
backend is chosen, the `StateStore` surface stays generic and the
backing decision is a Buerostack-wide architectural choice.

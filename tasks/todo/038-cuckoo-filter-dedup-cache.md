# 038 — Cuckoo-filter cache primitive for dedup / negative lookups

## Filed

2026-07-15 — generic framework primitive. Many DSL patterns
(dedup on ingest, negative-cache before a DB lookup, "have I already
processed this id" checks) want set-membership answered in <1 µs
without hitting persistent storage. The framework offers no such
primitive today.

## Problem

Some DSLs need to answer set-membership questions on high-cardinality
opaque ids:

- Dedup on ingest: "have I already ingested id `XYZ`?" — client
  retries frequently; per-request full DB lookup is wasteful.
- Idempotency-Key already has an in-process SHA256 store (see task 029
  for the durability follow-up) — but for pre-persistence dedup where
  losing state on restart is fine, a Cuckoo filter is 10-100× cheaper
  in memory and O(1) lookup.
- Negative caches: "definitely NOT a known id, skip the DB round-trip"
  — Bloom would work but Cuckoo supports deletion, which matters for
  TTL rotation.

## Fix

New step type (or new state-backend variant):

```yaml
dedup_check:
  cuckoo:
    op: check_and_add          # or: check / add / remove
    key: "ingest.dedup"        # namespaced cache name
    value: "${incoming.body.consignment_id}"
    into: was_seen             # binds bool
    capacity: 1000000          # tune per cache
    false_positive_rate: 0.001
  next: branch
```

Implementation: use the `cuckoofilter` crate (`0.5`, MIT). Store one
filter per cache name in a `DashMap<String, Arc<Mutex<CuckooFilter>>>`
inside StateStore (or a new CacheStore module).

## Not for

Anything durable. Cuckoo filters are process-local; a restart clears
them. Persist to Resql on the same event that triggered the "check
succeeded" branch, so the durability layer stays authoritative.

## Acceptance

- New `cuckoo` step type with `check`/`add`/`check_and_add`/`remove` ops.
- Configurable capacity + FPR (compile-time table size math).
- Concurrency: filter behind a `Mutex`; consider per-shard sharding
  if benchmarks show contention.
- Sample: `DSL/samples/POST/dedup-consignment.yml`.
- Bench in the perf suite (task 039) at 1 M unique ids: check throughput
  should be ≥100k ops/s per core.

## Alternatives considered

- Bloom filter: no deletion, TTL rotation clumsy.
- HashSet: 10× more memory per entry, no controllable FPR.
- Redis SISMEMBER: durable + shared, but adds a network hop + a
  dependency Ruuter otherwise doesn't require.

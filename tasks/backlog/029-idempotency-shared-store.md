# 029 — Idempotency-Key shared store for multi-instance deploys

## Why

`src/idempotency.rs` is an in-process `DashMap`. Two Ruuter replicas
behind a load balancer don't share the dedup cache — a client retry
that lands on a different pod re-runs the DSL. The PATTERNS.md §2
guarantee ("at-most-once semantics on writes across network retries")
holds only for a single-replica deploy.

## Status

**Excluded from this batch by owner — will be addressed properly
after everything else lands.**

## Acceptance sketch

- Pluggable backend trait: `trait IdempotencyBackend { get, insert }`.
- In-process backend (current DashMap) as the default.
- Redis backend behind a `redis` cargo feature; connection URL in
  config.
- Postgres backend behind a `postgres` cargo feature; single-table
  DDL migration ships as a Liquibase changeset alongside Resql's
  audit schema (PATTERNS.md §6).
- Race behaviour: `INSERT ... ON CONFLICT DO NOTHING` on Postgres,
  `SET NX EX <ttl>` on Redis.
- Integration test: two `DslRouter` instances sharing the same
  backend; a POST with `Idempotency-Key: X` to one replica followed
  by the same key to the other returns the same cached body with
  `Idempotency-Replayed: true`.

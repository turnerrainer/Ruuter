# 036 — Pool BoaContext per project instead of building per request

## Filed

2026-07-15 — surfaced by the 0.4.0 perf benchmark (see task 039).

## Status

**Blocked (2026-07-14)** — attempted on the v0.5 branch after task 037.
`boa_engine::Context` embeds `Rc<...>` internals and is therefore
`!Send + !Sync`. Storing it (even behind `Arc<Mutex<Option<...>>>`) on
`ExecutionContext` breaks the framework's `Send` contract across every
`.await` boundary — the supervisor, HTTP handlers, and source loops all
require `ExecutionContext: Send`. First attempt produced 289 compile
errors and was reverted.

Any workable form of this task requires ONE of:

1. **Dedicated OS worker threads for JS.** Spawn `num_cpus` `std::thread`s
   at startup, each owning a warm `BoaContext`. Async side dispatches
   `(script, bindings) → oneshot::Sender<Value>` via a channel; awaits
   the response. Boa never touches a tokio worker.
2. **`tokio::task::LocalSet` for DSL execution.** Would require the
   whole handler chain to opt out of the multi-threaded runtime for
   DSL steps. Large blast radius.
3. **`send_wrapper` crate.** Panics if the tokio task moves between
   worker threads — which it will under load. Not viable.

Approach #1 is the only one that composes with the current async
architecture and is what a future revision of this task should
prescribe. It also naturally caps concurrency (pool size = worker
threads), which the current per-request-context model does not.

Until then, task 037 (literal fast-path) is the shipping mitigation:
any DSL value with no `${...}` bypasses Boa entirely, so the pool
would only benefit values that actually invoke JS.

## Problem

`ScriptEngine::evaluate()` today builds a brand-new `BoaContext` on every
call, then calls `setup_bindings()` which:

1. Serializes `incoming.headers`/`body`/`params` via `serde_json::to_string`.
2. Injects them via `boa.eval(Source::from_bytes(&format!("var incoming = {};", ...)))`.
3. Re-injects every user variable the same way.

At `1,000 req/s` on a thin DSL (`/samples/basic/hello`) this is ~40% of
per-request CPU. `/health` (no DSL, no Boa) tops out at ~60,000 req/s
on 2 cores; the moment a DSL executes, throughput collapses to
~2,900 req/s. The framework itself is not the bottleneck.

## Numbers

Measured on this laptop, 2 CPU / 512 MB compose container:

| Endpoint | Runs | req/s | p50 lat |
|---|---|---:|---:|
| `/health` | axum only | 59,628 | 0.8 ms |
| `/samples/basic/hello` | 1 return step | 2,919 | 17.2 ms |
| `/samples/variables/complex-object` | assign + JS object literal | 1,856 | 26.9 ms |
| `/samples/things/abc/legs` | path params + switch | 1,287 | 38.8 ms |

The 20-50× cliff between framework and DSL is the ScriptEngine setup.

## Fix

Ship a per-project `BoaContext` pool (or ideally: reset a persistent
context via `Realm::create` between requests). Bind static values
(constants, project globals) ONCE at pool creation. Per-request:
only inject `incoming.*` and user vars into a fresh scope inside the
already-warm realm.

Acceptance:

- New module `src/scripting/pool.rs` with `BoaPool { project → Vec<BoaContext> }`.
- `ScriptEngine::evaluate()` checks out a context, sets a fresh scope,
  runs, resets scope, returns to pool.
- Handle Boa's realm/scope semantics correctly — if the pooled context
  ever leaks state between requests (e.g. a rogue `globalThis.x = 1`),
  it's a security bug.
- Pool size configurable via `scripting.pool_size_per_project` (default =
  `tokio::runtime::Runtime::num_workers()`).
- **Perf gate: throughput on `/samples/basic/hello` at least 3× the
  0.4.0 baseline (~9,000 req/s on the same 2-core box)**. If the pool
  doesn't get there, roll it back; the risk of state leakage isn't
  worth <3× gains.

## Non-goal

Sharing contexts across projects. Different projects have different
constant sets, different DSL trees, and different security postures.
Cross-project pooling would leak the constants file across tenants.

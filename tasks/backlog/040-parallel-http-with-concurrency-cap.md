# 040 — `parallel_http` step: bounded fan-out to N peers

## Filed

2026-07-15 — generic framework primitive; DSLs that need to broadcast
one inbound request to N peer services currently have no bounded way
to do it.

## Problem

The `iterate` step executes its body sequentially. `http` steps inside
`iterate` therefore serialize peer calls — fine for admin operations,
unsuitable for any DSL that fans one inbound request out to N peer
services when per-peer latency is 100-1000 ms and N is in the tens.

The alternative shape a DSL author reaches for — "spawn one request
per peer, all at once" — has a well-known failure mode: unbounded
outbound concurrency turns every inbound request into an N× amplifier
on downstream pools, thread limits, and the DSL's own tail latency.
The framework should offer a first-class bounded fan-out primitive so
consumers don't roll their own (or, worse, iterate serially and
accept the linear latency).

## Fix

New step type — `parallel_http`:

```yaml
fanout:
  parallel_http:
    call: http.get
    peers: "${gates}"                # array of peer records
    args:
      url: "${peer.baseUrl}/v1/identifiers/${incoming.body.uil}"
      headers: { Authorization: "Bearer [#peer_bearer]" }
      timeout: 2000                  # per-peer deadline, ms
    max_concurrency: 8               # bounded fan-out
    aggregate: collect_ok            # collect_ok | collect_all | first_n
    first_n: null                    # required when aggregate=first_n
    result: peer_responses           # array of {peer, response|error}
  next: assemble
```

Semantics:

- Spawns up to `max_concurrency` outbound requests at once from the
  `peers` array (uses `tokio::task::JoinSet` bounded by a `Semaphore`).
- Per-peer `timeout` shorter than the DSL's overall budget.
- `aggregate: collect_ok` — wait for all; discard errors; array of
  successful responses.
- `aggregate: collect_all` — wait for all; keep errors with a
  `{peer, error: "..."}` shape; array of everything.
- `aggregate: first_n` (see task 041) — return as soon as N peers
  succeed; cancel the rest.

## Acceptance

- New `ParallelHttpStep` in `src/steps/mod.rs`.
- Executor in `src/steps/parallel_http.rs` using `tokio::task::JoinSet`
  + `Arc<Semaphore>` for the cap.
- Per-peer timeout uses `tokio::time::timeout` around the individual
  `http_client.request` call.
- Cancellation of in-flight requests on `first_n` completion (`JoinSet`
  handles this via `abort_all` on drop of the outstanding handles).
- Result binding shape stable: `[{peer, response: {status, body, headers}}
  | {peer, error: "..."}]`.
- SSRF / size-cap / status-filter enforced per-peer identically to `http`.
- Traceparent forwarded per-peer identically to `http`.
- Integration test: `tests/parallel_http.rs` — 5 mock upstreams with
  varied latencies + failure mixes, verifies concurrency cap,
  aggregation modes, per-peer timeout, cancellation.

## Non-goal

Retries. Retry policy is per-peer and belongs in a wrapper (guard,
outer switch, or a separate `retry_http` step). Keep this step
composable.

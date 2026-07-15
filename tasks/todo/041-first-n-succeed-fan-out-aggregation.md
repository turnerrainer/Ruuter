# 041 — `first_n` aggregation mode for `parallel_http` (fan-out doesn't wait for stragglers)

## Filed

2026-07-15 — depends on task 040. Addresses eFTI Gate burst weakness B2
in `h2ck.me/projects/efti-gate-dlk/SCALABILITY.md` — DLK POC's own Test
Fest 3 evidence: **one slow peer → p95 = 60 s at 0.12 req/s** because
the broadcast waits for `flow.toList()` before responding.

## Problem

In cross-gate broadcasts, "correct answer" and "all peers replied" are
different. A regulator asking "does this UIL exist anywhere in the EU
network" wants the yes/no as soon as ANY authoritative peer answers —
not after every peer either replies or times out.

Naive "wait for all" turns a single slow peer into a 60 s tail-latency
tax on every query. Naive "return immediately" loses completeness.

## Fix

Extend `parallel_http` (task 040) with:

```yaml
fanout:
  parallel_http:
    peers: "${gates}"
    args: {...}
    max_concurrency: 8
    aggregate: first_n
    first_n: 1                       # return as soon as N peers reply
    early_exit_on:                   # short-circuit conditions
      status_range: [200, 299]       # only 2xx counts toward `first_n`
      body_predicate: "${response.body.found === true}"   # optional JS
    remaining_peers_after: cancel    # cancel | drain_bg | wait_bounded
    remaining_bounded_ms: 500        # only when remaining=wait_bounded
    result: peer_responses
  next: reply
```

Semantics:

- Count only peers matching `early_exit_on` toward `first_n`.
- When `first_n` reached, in-flight peers are dealt with per
  `remaining_peers_after`:
  - `cancel` — abort and drop (default for correctness-tolerant reads).
  - `drain_bg` — spawn a detached task to consume responses and update
    state (for cache warming / audit).
  - `wait_bounded` — wait up to `remaining_bounded_ms` for stragglers
    (gives partial completeness with a bounded tail).
- Recorded per response: `{peer, latency_ms, source: "early_exit" | "straggler" | "cancelled" | "timeout"}`.

## eFTI mapping

Regulator's "identifier query" pattern:

```yaml
lookup:
  parallel_http:
    peers: "${peer_gates}"
    args: { url: "${peer.baseUrl}/v1/identifiers/${incoming.body.uil}", timeout: 2000 }
    max_concurrency: 8
    aggregate: first_n
    first_n: 1
    early_exit_on:
      status_range: [200, 299]
      body_predicate: "${response.body.matches?.length > 0}"
    remaining_peers_after: drain_bg     # log everything for audit
    result: match
  next: reply
```

Turns a 60 s worst-case broadcast into a max-2 s early-exit path with
all late peers still audited.

## Acceptance

- `aggregate: first_n` implemented on top of task 040's `JoinSet`
  infrastructure (drop remaining handles on early exit → auto-cancel).
- `early_exit_on.body_predicate` evaluated via `ScriptEngine` in the
  scope of each incoming response.
- `remaining_peers_after: drain_bg` spawns a `tokio::spawn` with a
  bounded channel (avoid unbounded background growth).
- Integration test: 5 mock upstreams; slowest is 10 s; assert overall
  response completes in <500 ms with `first_n: 1` and cancels the 4
  remaining calls.
- Documented in `book/src/dsl/steps/parallel_http.md` (new).

## Non-goal

Quorum / voting semantics (Byzantine-style "N-of-M agree"). If
regulators later require this, it's a separate `aggregate: quorum`
mode; leave the extension point.

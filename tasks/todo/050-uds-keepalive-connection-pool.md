# 050 — UDS client keep-alive connection pool

## Filed

2026-07-19 — direct follow-up to task 043. v0.6.0 A/B bench found
UDS transport is ~6% SLOWER than TCP loopback on the sidecar-hop
workload, because reqwest's TCP client keep-alive pools warm
connections while my v1 UDS client does a fresh handshake per
request. Adding a pool to the UDS side closes the gap and delivers
the win 043's task predicted.

## Problem

`src/http_client/uds.rs::request_over_unix()` opens a fresh
`UnixStream` + calls `hyper::client::conn::http1::handshake()` on
EVERY request. That's:

- New file-descriptor per request
- New HTTP/1.1 connection state per request
- No pipelining across requests to the same target

Meanwhile the TCP path uses reqwest, which internally maintains a
per-host connection pool with keep-alive. Even on loopback, the
pooled TCP client wins because it amortises handshake cost across
many requests.

Measured on v0.6.0 (median-of-3, laptop, cross-instance sidecar
hop pattern):

| Transport | rps |
|---|---:|
| TCP loopback (pooled reqwest) | 4,229 |
| UDS via alias (per-request handshake) | 3,987 |

## Fix

Use `hyper-util::client::legacy::Client<C, B>` with a custom UDS
connector, which gives us the same pooling infrastructure reqwest
has for TCP.

### Sketch

```rust
use hyper_util::client::legacy::{Builder, Client};
use hyper_util::rt::TokioExecutor;

// Custom Connector<Uri> that maps a fake "unix://socket-path" URI
// to a Service<Uri, Response = UnixStream>. hyper-util has
// example patterns; also see `hyperlocal` crate for prior art.
struct UdsConnector { /* socket path lookup */ }

impl tower::Service<Uri> for UdsConnector {
    type Response = TokioIo<UnixStream>;
    // ... poll_ready, call() opens or reuses a stream
}

// Client with per-host pool
let client: Client<UdsConnector, Full<Bytes>> = Builder::new(TokioExecutor::new())
    .pool_idle_timeout(Duration::from_secs(30))
    .pool_max_idle_per_host(32)
    .build(UdsConnector { ... });
```

Wire this into `HttpClient` as `uds_client: Arc<Client<UdsConnector, Full<Bytes>>>`
alongside the reqwest client. `request_over_unix()` becomes a small
adapter that translates the alias/path to the fake URI and delegates
to `client.request(req).await`.

### Alternative: `hyperlocal` crate

`hyperlocal` (0.8+) is a known-good hyper 1.x UDS connector. Adds
one dep but skips writing the connector from scratch. Evaluate first
— it's ~200 loc; if it does exactly what we need, use it, else roll
our own.

## Acceptance

- `HttpClient` new field: `uds_client: Arc<...>` populated at
  construction, shared across clones via Arc.
- `uds::request_over_unix` reworked to use the pooled client;
  per-request handshake removed.
- Pool config surfaced in `AppConfig`:
  `http_client.uds_pool_idle_timeout_secs` (default 30) and
  `http_client.uds_pool_max_idle_per_host` (default 32).
- All existing UDS integration tests still pass byte-identically.
- New test: `tests/uds_pool.rs` — 100 sequential requests to the
  same UDS server complete in < 100ms total (proves pooling is
  actually reusing connections; without pooling this would take
  seconds).
- Bench: `bench/run-043-ab.sh` re-run shows UDS ≥ TCP-loopback
  on the sidecar-hop workload.

## Composability

- 049 (h2c): 050 first, then h2c on top. h2c client will need the
  same pool infrastructure — designing the pool cleanly makes h2c
  easier.
- 044 (self-call short-circuit): unaffected — self-calls don't
  touch the transport layer.

## Non-goals

- Cross-Ruuter-instance UDS pool sharing. Each Ruuter process has
  its own pool. If two instances hit the same target, they pool
  independently — the target handles multiple client pools fine.
- Explicit connection eviction API. Idle timeout is enough for a
  first version; if operators need finer control (evict a
  connection because a downstream restarted), file a follow-up.

## Risk

- Pool tuning: too-small `max_idle_per_host` means we thrash;
  too-large wastes fds. Defaults from hyper-util are reasonable
  but should be verified on the bench matrix.
- Stale-connection handling: if the downstream restarts, cached
  connections are dead. hyper-util handles this by removing failed
  connections from the pool on next-use, but the first request
  after a restart will see a failure and retry. Tests must cover
  the "target restarts mid-load" scenario.
- Fd exhaustion: 32 idle × N unique targets could exhaust ulimit on
  bench hosts. `bench/AWS-RUNBOOK.md` already recommends
  `ulimit -n 65535`.

# 043 — Unix Domain Socket transport for inter-service HTTP hops

## Filed

2026-07-16 — Ruuter's `HttpClient` is TCP-only, so any DSL that hops
to a sidecar on the same host pays TCP loopback cost even though the
packets never leave the machine. UDS is the standard fix.

## Problem

Every inter-process HTTP hop inside the trust boundary pays TCP
loopback cost:

- ~15-25 µs CPU per hop (netfilter, softirq, IP stack traversal)
- ~100-300 µs wall latency warm-conn
- Kernel TCP overhead compounds under sustained rps

Any deployment that runs Ruuter next to a sidecar on the same host
(Resql, TIM, custom adapters — the standard Buerostack composition
pattern) leaves this on the table. The network stack is doing
meaningful work on requests that never leave the machine.

## Fix

Support unix-socket transport in the outbound `HttpClient` and the
inbound axum listener:

### Outbound (`src/http_client/mod.rs`)

```rust
pub enum ClientTarget {
    Tcp(reqwest::Url),
    Unix { socket: PathBuf, path_and_query: String },
}

impl HttpClient {
    // ... existing constructors ...

    /// Route requests to `unix_socket_map` when the URL scheme is
    /// `unix://` OR when the host matches a configured mapping.
    pub fn with_unix_socket_map(mut self, map: HashMap<String, PathBuf>) -> Self {
        ...
    }
}
```

`reqwest` supports UDS via `hyper-util`'s `unix` feature — see
`reqwest::Client::builder()::http1_only().build()` combined with a
`hyper::client::conn::http1::Builder` over a `UnixStream`. Wrapper
kept behind the same `request(...)` API so DSL authors write the same
`http.<verb>` step regardless of transport.

### Inbound (main.rs / router build)

```yaml
# ruuter.yaml
listeners:
  http:  { bind: "0.0.0.0:8080" }              # external
  admin: { unix: "/var/run/ruuter/admin.sock" } # internal-only
```

`axum::serve` accepts any `impl Accept`; `tokio::net::UnixListener`
qualifies. Add a builder option to bind multiple listeners; the same
`Router` serves both.

### URL convention

```yaml
# DSL calling the internal Resql socket
fetch:
  call: http.get
  args:
    url: "unix:///var/run/ruuter/resql.sock/orders/latest"
    # or via alias in config:
    # url: "resql:///orders/latest"   # where `resql:` maps to the socket path
```

## Numbers to hit

Bench scenarios added to task 039's suite:

| Scenario | Baseline (TCP loopback) | Target (UDS) |
|---|---:|---:|
| Ruuter → mock-Resql sidecar, 1k rps | latency p50 | ≤ p50(TCP) − 150 µs |
| Ruuter → mock-Resql sidecar, 5k rps sustained | CPU % | ≤ CPU(TCP) − 3 pp |
| Ruuter → mock-AS4 sidecar, 1k rps | ↑ | ↑ |

Any scenario that regresses vs TCP fails the gate. UDS is an
optimization; it should not be slower than TCP even in edge cases.

## Acceptance

- `HttpClient` supports `unix://` URL scheme end-to-end.
- Config allows aliasing (`resql:` → `unix:///var/run/ruuter/resql.sock`).
- Axum multi-listener support: same router bound to both TCP and UDS
  concurrently.
- Integration test: DSL that does `http.get unix:///tmp/test.sock/...`
  round-trips against a same-process mock UDS server.
- Perf bench (039 suite) shows ≥150 µs p50 improvement + measurable
  CPU reduction under sustained load.
- Book chapter update: [`book/src/framework/inter-service-transport.md`](../../book/src/framework/inter-service-transport.md)
  (new) documenting when to use UDS vs TCP.

## Non-goals

- **UDS for external interfaces.** External clients hit Ruuter over
  TCP. UDS is for the process mesh inside the trust boundary
  (Ruuter ↔ Resql, Ruuter ↔ AS4 sidecar, Ruuter ↔ TIM, fronting
  proxy ↔ Ruuter when colocated).
- **HTTP/2 over UDS.** Nice-to-have but not required for the perf
  target. Files as a follow-up if the perf gate needs it.
- **Cross-node UDS.** By definition, UDS is single-node only. When
  Ruuter and Resql are on different nodes, UDS is not applicable;
  fall back to TCP with keep-alive.

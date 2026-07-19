# Inter-service transport (UDS)

Ruuter supports Unix Domain Sockets (UDS) for both outbound and inbound HTTP, letting on-host sidecar hops (Ruuter ↔ Resql, ↔ TIM, ↔ custom adapters) skip the TCP loopback stack.

## When to reach for UDS

- Ruuter and a sidecar (Resql, TIM, an AS4 MSH, an XTR adapter) run on the **same host**.
- The DSL does inter-service HTTP on the request-hot path (i.e. per-request, not startup-only).
- You want to keep DSLs portable — the same YAML runs under TCP-only deploys and UDS-enabled deploys.

Numbers: skipping netfilter + softirq + IP stack traversal typically saves **~15-25 µs CPU** and **~100-300 µs wall latency** per hop on Linux. On sustained-rps sidecar hops (5k+/sec) the CPU delta compounds into whole percentage points.

Measured on the v0.6.1 A/B bench (same-workload sidecar hop, 3-run median on a developer laptop):

| Transport | rps | p50 |
|---|---:|---:|
| TCP loopback (reqwest pooled) | 4,839 | 13.0 ms |
| **UDS via alias (pooled — task 050)** | **5,122** | **12.4 ms** |

UDS wins by ~6% throughput and ~5% latency. The v1 UDS (0.6.0, no pooling) was actually 6% SLOWER than pooled TCP loopback — task 050 added a hyper-util-based keep-alive pool per unique socket path, closing the gap.

## When NOT to reach for UDS

- Cross-node hops. UDS is single-host by definition; over the wire you're back on TCP.
- External clients — you should not expose UDS as an ingress from outside the trust boundary.
- Sub-1k rps DSLs where the loopback overhead is noise relative to the DSL body.

## Outbound

Two ways for a DSL to hit a socket:

### Option A (recommended): alias map in config

```yaml
# ruuter.yaml
unix_socket_map:
  resql:  "/var/run/ruuter/resql.sock"
  tim:    "/var/run/ruuter/tim.sock"
```

Then in the DSL, write a plain-looking HTTP URL whose host matches an alias:

```yaml
fetch_orders:
  call: http.get
  args:
    url: "http://resql/query/orders/latest"        # transparently UDS
    headers: { "content-type": "application/json" }
  result: upstream
```

The DSL is portable: the same file runs under TCP-only (host `resql` doesn't resolve → the call fails, and the operator adds the alias) and under UDS-enabled deploys (host `resql` matches → UDS transport, DSL unchanged).

### Option B: explicit `unix://` URL

```yaml
fetch_orders:
  call: http.get
  args:
    url: "unix:///var/run/ruuter/resql.sock/query/orders/latest"
  result: upstream
```

Use this only when you want to make the transport choice explicit in the DSL. It couples the DSL to the socket path — swap between TCP and UDS deployments requires editing the YAML.

URL parsing: the socket-path segment ends at the first `.sock` or `.socket` marker. Everything after is the request-line target. If neither marker appears, the parser falls back to splitting after the first path segment past the leading `///` — imprecise, so stick to `.sock` / `.socket` extensions in production.

### Interactions

- **SSRF allow-list** (`internal_requests.allowed_urls` / `allowed_ips`): applies to TCP URLs only. UDS bypasses these — a Unix path is not a remote address, and permitting the socket is the operator's ambient authority. If you need to gate DSL access to a specific socket, control who can write DSLs that mention it (or don't add the alias in the first place).
- **Timeout**: honored end-to-end (connect + request + response body).
- **`http_codes_allow_list`**: honored (checked after the response arrives).
- **`http_response_size_limit`**: honored, but the check is post-hoc (whole body collected first). For hard-cap streaming, use the TCP path or file a follow-up.
- **Response headers, request headers, JSON body**: same shape as TCP.

## Inbound

Add a `listeners:` section to `ruuter.yaml`. When present, it **replaces** the default single TCP listener on `port`.

```yaml
# ruuter.yaml
port: 8080                       # ignored when `listeners` is non-empty

listeners:
  - name: public
    bind: "0.0.0.0:8080"          # external ingress

  - name: internal
    unix: "/var/run/ruuter/admin.sock"    # sidecar / control plane
```

Every listener serves the **same axum Router** — the same routes, the same guards, the same OpenAPI spec, the same CSRF/CORS/Idempotency-Key behaviour. Only the transport differs.

Rules and gotchas:

- Exactly one of `bind` or `unix` per listener. Both-or-neither is a startup error.
- Stale socket files (left behind by a crashed prior instance) are removed before bind.
- Every listener runs its own accept loop; if any listener's task panics, the process exits (fail-fast rather than silently drop traffic on one transport).
- File permissions on the socket default to the umask. To restrict access to a specific group, wrap the socket path in a mode-controlled directory OR use a `SocketPermissions` future extension.

## HTTP/2 (h2c) over UDS (task 049)

v0.6.2 adds h2c on both outbound and inbound UDS paths.

```yaml
# outbound
uds_http_version: http2      # default http1

# inbound listener
listeners:
  - name: side-uds
    unix: "/var/run/ruuter/side.sock"
    http2: true              # default false
```

Both sides must speak the same version — no ALPN over cleartext.

**Measured A/B** (v0.6.2, laptop, sequential-per-request workload):

| Version | rps | p50 |
|---|---:|---:|
| h1 with pool (task 050) | 5,121 | 12.4 ms |
| h2c (task 049) | 4,910 | 13.0 ms |

**h2c is ~4% slower here.** h2 multiplexing only pays off when a single caller issues many concurrent streams to the SAME target. A DSL that makes one upstream call per inbound request never engages multiplexing — each of wrk's 64 connections issues one request at a time, so h1 with pooling and h2 with one-stream-per-request perform equivalently, with h2 losing on per-frame overhead.

**Where h2c will pay off**: once a `parallel_http` primitive (task 040, backlog) fans one inbound request out to N concurrent upstreams. One h2 connection carries N streams; h1 needs N pooled connections. Expected win: 3-5× on fan-out.

**Recommendation until 040 lands**: keep the default `http1`. Ship h2c as opt-in for operators already using fan-out patterns via `iterate` around `http.<verb>`.

## Non-goals

- **Cross-node UDS**: impossible by definition. When Ruuter and a sidecar are on different nodes, TCP with keep-alive is the answer.
- **UDS as external ingress**: the framework doesn't stop you, but it's outside the design intent. External clients hit TCP.
- **HTTPS/h2 with ALPN**: external-facing TLS termination is deployment-layer's job (nginx, envoy, ALB). Ruuter stays cleartext.
- **HTTP/3 (QUIC)**: file separately if a compelling workload emerges.
- **Streaming request-body size caps on UDS**: today the UDS response-body cap is post-hoc (buffer, then check). Streaming enforcement is a follow-up.

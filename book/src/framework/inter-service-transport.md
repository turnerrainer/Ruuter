# Inter-service transport (UDS)

Ruuter supports Unix Domain Sockets (UDS) for both outbound and inbound HTTP, letting on-host sidecar hops (Ruuter ↔ Resql, ↔ TIM, ↔ custom adapters) skip the TCP loopback stack.

## When to reach for UDS

- Ruuter and a sidecar (Resql, TIM, an AS4 MSH, an XTR adapter) run on the **same host**.
- The DSL does inter-service HTTP on the request-hot path (i.e. per-request, not startup-only).
- You want to keep DSLs portable — the same YAML runs under TCP-only deploys and UDS-enabled deploys.

Numbers: skipping netfilter + softirq + IP stack traversal typically saves **~15-25 µs CPU** and **~100-300 µs wall latency** per hop on Linux. On sustained-rps sidecar hops (5k+/sec) the CPU delta compounds into whole percentage points.

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

## Non-goals

- **HTTP/2 over UDS**: nice-to-have, not shipped. HTTP/1.1 with keep-alive covers the typical sidecar pattern.
- **Cross-node UDS**: impossible by definition. When Ruuter and a sidecar are on different nodes, TCP with keep-alive is the answer.
- **UDS as external ingress**: the framework doesn't stop you, but it's outside the design intent. External clients hit TCP.
- **Streaming request-body size caps on UDS**: today the UDS response-body cap is post-hoc (buffer, then check). Streaming enforcement is a follow-up.

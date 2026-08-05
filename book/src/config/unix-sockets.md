# Unix-socket aliases

DSL-transparent outbound UDS routing. The DSL keeps writing
`http://<alias>/…` while the operator swings the transport from TCP
loopback to a Unix socket via config only.

Full behaviour reference in [Framework — Inter-service transport (UDS)](../framework/inter-service-transport.md).

## What it is

`unix_socket_map` maps `http://<host>/…` URLs whose **host component**
matches a key onto a UDS transport hitting the mapped socket path.
The DSL is oblivious; the same YAML runs unchanged under TCP-only
deploys and UDS-enabled deploys.

## The config

```yaml
unix_socket_map:
  resql: /var/run/ruuter/resql.sock
  tim:   /var/run/ruuter/tim.sock

uds_http_version: http1        # or `http2` for h2c on outbound UDS
```

## The defaults and why

- `unix_socket_map: {}` — empty. The alias mechanism is opt-in; no
  hidden magic at parse time.
- `uds_http_version: http1` — HTTP/1.1 with keep-alive. Every
  sidecar can speak it, so the safe default. Flip to `http2` when
  every UDS-reachable sidecar has been proven h2c-capable — h2c
  stream multiplexing measurably raises throughput on
  concurrent-request workloads.

## When to use

- You want to migrate a hot-path sidecar hop off TCP loopback
  without touching a single DSL file.
- You need portable DSLs — the same file must work in a dev
  environment that has no UDS and in a prod deploy that does.
- You're worried about accidentally exposing `unix://` URLs to a
  DSL that gets used elsewhere; aliases stay grep-able in the config
  and hidden from the DSL.

The `unix://` scheme is also supported directly in DSL URLs. Aliases
are the preferred form because they keep DSLs portable.

## What breaks if you set it wrong

- Alias name collides with a real DNS host you also need to reach →
  the alias wins. Rename the alias (e.g. `resql-uds`).
- Socket path points at a file Ruuter can't reach (permissions,
  wrong mount) → every request through the alias errors with a
  connect failure. Test with `curl --unix-socket <path>`.
- `uds_http_version: http2` when the sidecar speaks only h1 → every
  request fails ALPN. Return to `http1`.
- `internal_requests.disabled: true` → alias requests are blocked
  too; the disabled guard runs above the transport dispatch.
- `internal_requests.allowed_urls` non-empty that doesn't list the
  alias origin → alias requests get rejected as "not in allowed_urls".
  Add the alias-form origin (`http://<alias>`) to the list.

## Copy-clean YAML

```yaml
unix_socket_map:
  resql: /var/run/ruuter/resql.sock

uds_http_version: http1
```

DSL step, unchanged from the TCP-loopback version:

```yaml
fetch:
  http.get:
    url: http://resql/query?id=42
  next: end
```

## Cross-links

- [Framework — Inter-service transport (UDS)](../framework/inter-service-transport.md)
- [Listeners](./listeners.md) — the inbound side
- [Internal-requests](./internal-requests.md)

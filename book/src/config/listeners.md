# Listeners

Multiple inbound listeners — expose the same axum router over TCP,
UDS, or both, with h2c on either.

## What it is

By default Ruuter binds one TCP listener on `port` (from the top-level
`port:` field). Setting `listeners:` **replaces** that default with an
explicit list. Every listener serves the same routes; they differ only
in transport and where they bind.

## The config

```yaml
listeners:
  - name: public
    bind: 0.0.0.0:8080
  - name: internal
    unix: /var/run/ruuter/internal.sock
    http2: true
```

Exactly one of `bind` or `unix` must be set per entry — both-or-neither
is a config error caught at boot. `name` is optional; it labels the
listener in startup logs.

## The default and why

Empty. The vast majority of deployments run one TCP listener; the
top-level `port` field covers that case without ceremony. Multiple
listeners are opt-in for the specific cases below.

## When to use

- **Fast-path sidecar traffic**: expose an admin-only surface on a
  UDS so a co-located sidecar can hit it without touching TCP or DNS.
- **h2c for high-fanout callers**: pair a UDS listener with
  `http2: true` and an h2c-speaking sidecar to stream-multiplex
  bursty request patterns.
- **Split-horizon binds**: TCP on `0.0.0.0:8080` for external clients,
  TCP on `127.0.0.1:8081` for a local-only admin endpoint the
  operator scrapes via curl over ssh.

## Semantics

- One accept loop per entry, all sharing the same `axum::Router`.
- UDS paths that already exist on disk are removed before binding
  (stale-socket cleanup after a crashed prior instance).
- `http2: true` swaps hyper's http1 server builder for the http2
  builder on that listener. Http1 clients then fail the ALPN
  negotiation — pick one per listener.
- Removing the whole `listeners:` block reverts to `0.0.0.0:<port>`
  as the sole listener (pre-`listeners:` behaviour).

## What breaks if you set it wrong

- Both `bind` and `unix` on the same entry → boot fails with a
  descriptive config error.
- Neither `bind` nor `unix` → same failure.
- A UDS `unix:` path in a directory Ruuter can't write to → boot
  fails at listener setup.
- `http2: true` on a listener whose only clients speak http1 → every
  request fails the ALPN handshake.

## Copy-clean YAML — TCP + UDS

```yaml
listeners:
  - name: public
    bind: 0.0.0.0:8080
  - name: admin
    unix: /var/run/ruuter/admin.sock
```

## Copy-clean YAML — TCP only, non-default port

```yaml
port: 8181
# listeners: omitted → single TCP listener on 0.0.0.0:8181
```

## Cross-links

- [Framework — Inter-service transport (UDS)](../framework/inter-service-transport.md)
- [Unix-socket aliases](./unix-sockets.md) — the outbound side
- [Configuration overview](../ops/configuration.md)

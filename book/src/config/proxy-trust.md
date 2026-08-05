# Reverse-proxy trust list

Names the IP addresses whose `X-Forwarded-For` / `X-Real-IP` headers
Ruuter is willing to trust when it decides what `incoming.origin`
should be.

## What it is

`incoming.origin` is the field DSL code uses for audit lines, rate
limits, and self-call bookkeeping. When the request arrives from a
trusted reverse proxy, `origin` should reflect the **real** client;
when it arrives from an untrusted peer, `origin` must be the socket
peer — otherwise any curl caller can spoof it via a header.

## The config

```yaml
proxy:
  trusted: []                # list of peer IPs allowed to set XFF / X-Real-IP
```

## Semantics

- **Empty list (default)** — no proxy is trusted. `incoming.origin`
  is always the socket peer. Safe for direct-exposed deployments.
- **Non-empty list** — for each incoming request:
  - If the direct TCP peer's IP is in the list, the framework reads
    `X-Forwarded-For` (leftmost hop only — post-v0.7) or `X-Real-IP`
    and promotes it into `incoming.origin`.
  - If the direct peer isn't listed, the headers are still visible via
    `incoming.headers` (some DSLs hash them for logging), but
    `incoming.origin` reflects the socket peer.

IPv6-mapped IPv4 addresses (`::ffff:1.2.3.4`) are canonicalised to
`1.2.3.4` before the trust comparison, so operators write plain IPv4
in the list even when their proxy dials in over IPv6.

## The default and why

Empty. The safest posture for the majority of deployments (no proxy
in front) is "trust nobody". Operators who **do** front Ruuter with
nginx / envoy / an ALB add the proxy's private-network IP explicitly.

## What breaks if you set it wrong

- Listing a proxy IP that isn't actually the direct peer (e.g.
  because a load balancer sits between the proxy and Ruuter) →
  `incoming.origin` is still the socket peer (the load balancer),
  not the client. Correct fix: list the load balancer's IP, not the
  upstream proxy's.
- Listing a public untrusted IP → any request from that IP can spoof
  `incoming.origin` via `X-Forwarded-For`. Don't.
- Empty list when a proxy IS in front → `incoming.origin` collapses
  to the proxy's IP for every request. Rate-limit / audit output
  becomes useless. Add the proxy's IP.

## Copy-clean YAML

```yaml
proxy:
  trusted:
    - 10.0.0.5           # in-cluster nginx
    - 10.0.0.6           # in-cluster envoy
```

## Cross-links

- [Framework — Request pipeline](../framework/pipeline.md)
- [Configuration overview](../ops/configuration.md)
- [Security hardening checklist](../ops/security-checklist.md)

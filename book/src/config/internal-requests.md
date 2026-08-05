# Internal-requests (SSRF allowlist)

Controls what URLs the [`http` step](../dsl/steps/http.md) may reach.
Covers the kill-switch, the explicit allow-list, and the
private-network default-deny.

Full behaviour reference in [Framework — SSRF allow-list](../framework/ssrf.md).

## What it is

Every outbound HTTP call from a DSL passes through four gates. The
first that trips returns an `http` step error to the caller.

## The config

```yaml
internal_requests:
  disabled: false               # kill-switch for ALL outbound HTTP
  allowed_urls: []              # URL prefixes or exact origins
  allowed_ips: []               # bare-IP host allow-list
  block_private_networks: true  # default-deny RFC-1918 / loopback / link-local / ULA
```

## The defaults and why

- `disabled: false` — outbound HTTP is on by default. Ops flip to
  `true` for locked-down deploys (e.g. an isolated read-only replica
  that must never call sidecars).
- `allowed_urls: []` — empty means "no explicit allow-list". Combined
  with the private-network gate below, the default posture is: any
  public host is OK, no private host is.
- `allowed_ips: []` — same posture; bare-IP DSL calls are gated by
  the private-network check when this list is empty.
- `block_private_networks: true` — default-deny for
  RFC-1918 / loopback / link-local / CGNAT / ULA targets. Enforced
  **after** DNS resolution so a public-looking hostname that resolves
  to `127.0.0.1` still gets blocked (rebinding defence). Cloud
  metadata (`169.254.169.254`) is covered by the link-local rule.

## Kill-switch semantics

`internal_requests.disabled: true` rejects at the top of
`HttpClient::request`, **before** any transport dispatch. That means:

- TCP outbound → blocked.
- `unix://` scheme URLs → blocked.
- `unix_socket_map` alias URLs → blocked.
- Self-call short-circuit URLs → blocked.

This closes the pre-v0.7 gap where `disabled: true` only intercepted
the TCP path.

## What breaks if you set it wrong

- `disabled: true` on a DSL that legitimately needs a sidecar hop →
  every dependent request 500s with `outbound HTTP is disabled…`.
- `block_private_networks: true` (default) with a co-located sidecar
  reached via `http://127.0.0.1:8181/…` → the private-network check
  rejects it. Fix by either adding the sidecar's exact origin to
  `allowed_urls` OR moving the hop to a UDS alias (see
  [Unix-socket aliases](./unix-sockets.md)).
- `allowed_urls` written as a substring rather than exact origin —
  post-v0.7 the framework requires exact `scheme://host:port` match
  on bare-origin entries. `http://api.example.com` no longer admits
  `http://api.example.com.evil.tld`.

## Copy-clean YAML

Same-host sidecar exempted from the private-network default, external
API pinned by exact origin:

```yaml
internal_requests:
  disabled: false
  allowed_urls:
    - http://127.0.0.1:8181
    - https://api.partner.example.com
  allowed_ips: []
  block_private_networks: true
```

## Cross-links

- [Framework — SSRF allow-list](../framework/ssrf.md)
- [Framework — Inter-service transport (UDS)](../framework/inter-service-transport.md)
- [Unix-socket aliases](./unix-sockets.md)
- [http step](../dsl/steps/http.md)

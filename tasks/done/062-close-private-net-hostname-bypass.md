# 062 — `block_private_networks` misses hostname-encoded private targets (F2)

## Filed

2026-07-20 — surfaced by h2ck.me second-round "break-the-fix"
sweep (finding F2, `POST-FIX-REVIEW-2.md`). Follows task 060.

## Severity

**High**. The N4 blocklist's stated goal — "outbound TCP requests
targeting link-local / loopback / private ranges are rejected
before dispatch" — is fully bypassed by a caller that uses a DNS
name instead of an IP literal. Cloud-metadata SSRF via
`http://metadata.google.internal/`,
`http://metadata.gce.internal/`,
`http://kubernetes.default.svc.cluster.local/`, or plain
`http://localhost/` is not blocked by the default config.

## Reproduction

```rust
// tests/security_new_probes_2.rs::break_n4_hostname_to_private_ip_bypasses_blocklist
// Default config: block_private_networks: true, empty allowlists.
// DSL calls: http://localhost:<victim_port>/latest/meta-data/...
// Victim listener on 127.0.0.1:<victim_port> receives the request.
```

Currently PASSES (the assertion is "reached == true" — the bypass
works).

## Root cause

`src/http_client/mod.rs:311-330`:

```rust
if self.block_private_networks
    && self.allowed_url_prefixes.is_empty()
    && self.allowed_ip_hosts.is_empty()
{
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str()?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_local(ip) {
            return Err(...);
        }
    }
    // ← hostname falls through; no error returned.
}
```

`parsed.host_str()` returns the URL's HOST TEXT (`"localhost"`,
`"metadata.google.internal"`), which does not parse as `IpAddr`.
The check is silently skipped, `check_ssrf` returns `Ok(())`, and
the request is dispatched. Hyper's DNS resolver then converts the
name to a private address on the wire.

## Fix — recommended: resolve + verify

Before dispatching, resolve the URL host via
`tokio::net::lookup_host("<host>:<port>")` and apply
`is_private_or_local` to every returned `SocketAddr`. Reject on
any private hit. Then pin the outbound connection to the filtered
IP so DNS-rebinding between check and connect is defeated.

Sketch:

```rust
// src/http_client/mod.rs — new async helper called from the
// existing check_ssrf branch when block_private_networks is on.
if self.block_private_networks
    && self.allowed_url_prefixes.is_empty()
    && self.allowed_ip_hosts.is_empty()
{
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str().ok_or(...)?;
    let port = parsed.port_or_known_default().ok_or(...)?;
    // Fast path — the host is already an IP literal.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_local(ip) {
            return Err(RuuterError::HttpRequest(format!(
                "outbound to private target '{}' blocked (block_private_networks)",
                host
            )));
        }
    } else {
        // Slow path — resolve and check every candidate.
        let addrs = tokio::net::lookup_host(format!("{}:{}", host, port)).await
            .map_err(|e| RuuterError::HttpRequest(
                format!("dns lookup for '{}' failed: {}", host, e)))?;
        for a in addrs {
            if is_private_or_local(a.ip()) {
                return Err(RuuterError::HttpRequest(format!(
                    "outbound to '{}' rejected: DNS resolves to private/link-local {}",
                    host, a.ip()
                )));
            }
        }
        // Optionally: pin the reqwest connection to the resolved IP
        // to defeat DNS rebinding between check and connect. reqwest
        // exposes `resolve` on ClientBuilder — see docs.
    }
}
```

`check_ssrf` becomes `async` — call sites in `request()` already
await; this is a mechanical change.

## Fix — fast stop-gap

If the async plumbing above is bigger than the release window
allows, ship an interim: when `block_private_networks` is on and
no allowlist is configured, reject any URL whose host does not
parse as an IP literal.

```rust
if let Ok(ip) = host.parse::<IpAddr>() {
    if is_private_or_local(ip) {
        return Err(...);
    }
} else {
    return Err(RuuterError::HttpRequest(format!(
        "outbound to hostname '{}' rejected under block_private_networks — \
         add an entry to internal_requests.allowed_urls or allowed_ips to opt in",
        host
    )));
}
```

Trade-off: legitimate external hostnames (`api.stripe.com`) now
require an explicit allowlist entry. That's the correct
defense-in-depth default; internal DSLs already need allowlists for
same-service integrations.

## Acceptance

- `tests/security_new_probes_2.rs::break_n4_hostname_to_private_ip_bypasses_blocklist`
  must flip: the victim listener MUST NOT be reached.
- Add positive test: `allowed_urls: ["http://api.stripe.com/"]`
  permits an outbound to that hostname even with
  `block_private_networks: true`.
- Add negative test: `http://metadata.google.internal/` (or any
  hostname that resolves to 169.254.x.x) is rejected on a stack
  where the resolver produces a private address.
- Existing `default_config_permits_link_local_metadata_target`
  probe: originally pinned the current buggy default. After this
  fix + task 060 combined, that pin flips too.

## Non-goals

- IPv6-only hostname handling — the same fix applies; test with a
  hostname that resolves only to `::1`.
- Full DNS-rebinding-resistant connection pool — reqwest's
  `resolve()` helper is sufficient for the initial connect; if
  reqwest reuses connections across `.get(...)` calls to the same
  host, rebinding within a single connection is out of scope
  (reqwest resolves per connection, not per request).

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-2.md § F2`
- Task 060 (the fix this extends).

Effort estimate: 45 min for the async plumbing + tests; 15 min for
the stop-gap.

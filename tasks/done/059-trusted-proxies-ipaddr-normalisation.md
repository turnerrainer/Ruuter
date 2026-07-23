# 059 — Compare `proxy.trusted` entries as `IpAddr`, not strings (h2ck.me N3, residual S4)

## Filed

2026-07-20 — surfaced by h2ck.me post-fix follow-up sweep
(finding N3 — design pin, `POST-FIX-REVIEW.md`). Pinned by
`tests/security_new_probes.rs::s4_ipv6_mapped_ipv4_string_mismatch_drops_trust`.

## Severity

**Low** — fails **closed** (XFF adoption is skipped, `origin`
falls back to the socket peer). Not exploitable — a deployment
footgun. Legitimate operators lose XFF adoption when their peer
address changes between IPv4 and IPv4-mapped-IPv6 representation.

## Problem

`src/router/mod.rs:800-802`:

```rust
let peer_is_trusted = match &peer_ip_str {
    Some(ip) => trusted_proxies.iter().any(|t| t == ip),
    None => false,
};
```

`peer_ip_str` is `SocketAddr::ip().to_string()`. On a dual-stack
socket the same peer may arrive as `127.0.0.1` or
`::ffff:127.0.0.1` depending on the connecting stack and axum's
listener wiring. An operator who writes `trusted: ["127.0.0.1"]`
silently loses XFF adoption when the peer arrives IPv6-mapped.

Same class of bug in reverse: an operator on an IPv6-first stack
writing `trusted: ["::1"]` won't trust a v4 loopback peer, even
though semantically they refer to the same "localhost".

## Fix

Parse both sides as `std::net::IpAddr`. Canonicalise IPv4-mapped
IPv6 (`::ffff:a.b.c.d`) to plain IPv4 using
`Ipv6Addr::to_ipv4_mapped()`, then compare as `IpAddr`.

```rust
use std::net::IpAddr;

fn canonicalise(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 @ IpAddr::V4(_) => v4,
    }
}

let peer = peer_addr.map(|p| canonicalise(p.ip()));
let trusted: Vec<IpAddr> = trusted_proxies
    .iter()
    .filter_map(|s| s.parse::<IpAddr>().ok().map(canonicalise))
    .collect();
let peer_is_trusted = peer.is_some_and(|p| trusted.contains(&p));
```

A trusted entry that doesn't parse as an IP is silently dropped
(and logged at debug). This is the safer default; a subsequent
task can add a boot-time warning for unparseable entries.

Consider parsing `trusted` entries ONCE at boot (in `ProxyConfig`
validation) and caching as `Vec<IpAddr>` on the router — avoids
per-request parsing.

## Acceptance

- `resolve_origin` compares peer and trusted list as `IpAddr`,
  with IPv4-mapped-IPv6 canonicalised to IPv4.
- Flip
  `tests/security_new_probes.rs::s4_ipv6_mapped_ipv4_string_mismatch_drops_trust`
  to assert XFF IS adopted when `trusted: ["::ffff:127.0.0.1"]`
  and the peer arrives as `127.0.0.1`. Rename to
  `s4_trusted_proxies_normalise_ipv4_mapped_ipv6`.
- Add positive test: `trusted: ["127.0.0.1"]`, peer arrives as
  `::ffff:127.0.0.1` — XFF adopted.
- Add negative test: `trusted: ["not-an-ip"]` — entry is dropped,
  no trust granted, no panic.
- Add positive test: `trusted: ["::1"]`, peer is `::1` — XFF
  adopted (loopback IPv6 baseline still works).

## Non-goals

- CIDR entries (`trusted: ["10.0.0.0/8"]`) — file as a follow-up
  if operators ask; not part of this fix.
- DNS-based trusted lists — same as above.

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW.md § N3`
- `projects/Ruuter-on-Rust/REVIEW.md § S4` (parent, already fixed)
- Related: task 058 (leftmost XFF value) — same file, bundle.

Effort estimate: 30 min including tests + boot-time parse-and-cache.

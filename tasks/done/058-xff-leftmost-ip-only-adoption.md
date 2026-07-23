# 058 — Adopt leftmost XFF value as IP-only, not the whole chain (h2ck.me N2, residual S4)

## Filed

2026-07-20 — surfaced by h2ck.me post-fix follow-up sweep
(finding N2, `POST-FIX-REVIEW.md`). Pinned by
`tests/security_new_probes.rs::s4_residual_xff_chain_adopted_whole_string_not_leftmost`.

## Severity

**Low** — the S4 fix already gates XFF adoption on
`proxy.trusted`, so a random internet caller can't spoof
`origin`. This is the second-order concern: a caller behind a
legitimate trusted proxy still fully controls the leftmost XFF
value, and adopting the whole chain (rather than just the
leftmost IP) means:

- A DSL reading `${incoming.origin}` as a client identifier gets
  a comma-separated string, not an IP — brittle for hashing or
  ACL comparison.
- If the DSL splits on `,` and takes `[0]`, the attacker fully
  controls what shows up.
- Non-IP tokens in the leftmost slot (e.g.
  `X-Forwarded-For: attacker.example, 10.0.0.1`) currently
  propagate as `origin` verbatim.

## Problem

`src/router/mod.rs:804-810`:

```rust
if peer_is_trusted {
    if let Some(v) = headers_map
        .get("x-forwarded-for")
        .or_else(|| headers_map.get("x-real-ip"))
    {
        return v.clone();     // whole chain, including caller-controlled leftmost
    }
}
```

## Fix

Split the header on `,`, trim whitespace, take the leftmost token
that parses as an `IpAddr`. If the leftmost token doesn't parse
as an IP, treat the XFF as spoofed / malformed and fall back to
the socket peer.

```rust
if peer_is_trusted {
    if let Some(raw) = headers_map
        .get("x-forwarded-for")
        .or_else(|| headers_map.get("x-real-ip"))
    {
        // RFC 7239 / de-facto XFF semantics: leftmost is the
        // original client; each proxy appends its own address as
        // it forwards. We adopt the leftmost value, validated as
        // an IP. A non-IP leftmost is a misconfigured proxy or a
        // spoof attempt — fail back to the socket peer.
        if let Some(first) = raw.split(',').next() {
            let trimmed = first.trim();
            if trimmed.parse::<std::net::IpAddr>().is_ok() {
                return trimmed.to_string();
            }
        }
    }
}
```

`X-Real-IP` is a single value by convention but the same parse
gate applies — reject if it's not an IP.

## Acceptance

- `resolve_origin` returns only the leftmost IP-parseable token
  from XFF (or the peer IP if the leftmost isn't an IP).
- Flip
  `tests/security_new_probes.rs::s4_residual_xff_chain_adopted_whole_string_not_leftmost`:
  send `X-Forwarded-For: 198.51.100.7, 10.0.0.1` from a trusted
  peer, expect `origin_seen == "198.51.100.7"` (not the whole
  chain). Rename the test to
  `xff_from_trusted_peer_adopts_leftmost_ip_only` and update the
  doc-comment.
- Update
  `tests/security_hardening.rs::xff_from_trusted_peer_is_adopted_as_origin`
  to match the new contract if it still asserts the whole chain.
- Add negative test: trusted peer sends
  `X-Forwarded-For: attacker.example, 10.0.0.1` — `origin` MUST
  be the socket peer (leftmost isn't an IP → fall back).

## Non-goals

- Full RFC 7239 `Forwarded:` header parser — that's a separate
  task if we want the multi-parameter form.
- IPv6 zone-id normalisation — the parser will accept a bare
  IPv6 form; zone IDs (`%eth0`) are rare in XFF.

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW.md § N2`
- `projects/Ruuter-on-Rust/REVIEW.md § S4` (parent, already fixed)
- Related: task 059 (dual-stack IP compare) — same file, same
  function neighbourhood. Bundle if scheduling allows.

Effort estimate: 20 min including tests.

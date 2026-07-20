# 064 — DNS lookup timeout in `check_ssrf` (F2 addendum, DoS surface)

## Filed

2026-07-20 — surfaced by h2ck.me third-round follow-up
(`POST-FIX-REVIEW-3.md` § N-Info-2). Follows task 062.

## Severity

**Low** — DoS surface only, no bypass. `tokio::net::lookup_host`
inherits the system resolver's default timeout (typically 30s on
glibc, longer with misbehaving upstream resolvers). A DSL that
fans out 100 concurrent `http.get` calls to slow-DNS hosts ties up
100 tokio tasks for the resolver default.

## Problem

`src/http_client/mod.rs::check_ssrf` (F2 branch, roughly `:357-361`)
calls `tokio::net::lookup_host(...)` with no wrapping timeout.
The reqwest client has `default_timeout` set from
`http_request_timeout` in `AppConfig` — the DNS lookup should
respect the same budget so the operator has one timeout to tune.

## Fix

Wrap the lookup in `tokio::time::timeout(self.default_timeout, ...)`.
Return a clear "DNS lookup for '{host}' timed out after Ns" error
on elapse.

```rust
// src/http_client/mod.rs, inside check_ssrf hostname branch
let addrs = tokio::time::timeout(
    self.default_timeout,
    tokio::net::lookup_host(lookup_target.as_str()),
)
.await
.map_err(|_| RuuterError::HttpRequest(format!(
    "dns lookup for '{}' timed out after {}ms",
    host, self.default_timeout.as_millis()
)))?
.map_err(|e| RuuterError::HttpRequest(format!(
    "dns lookup for '{}' failed: {}", host, e
)))?;
```

## Acceptance

- Existing hostname tests must still pass unchanged
  (`break_n4_hostname_to_private_ip_bypasses_blocklist`,
  `f2_positive_allowlisted_hostname_passes_even_with_blocklist_on`).
- Add a test that patches `AppConfig::http_request_timeout` down to
  something like 100ms and issues a call to a hostname whose
  resolver is deliberately slow (harder in a hermetic test — a
  test-only DNS stub or a non-routable TLD like `.invalid.example`
  works as a proxy for "resolver takes forever, then fails"). Assert
  the error mentions `"timed out"`.

## Non-goals

- Full async-resolver replacement (Ruuter deliberately uses the
  system resolver; `trust-dns-resolver` is out of scope).
- Cache-layer for DNS responses (leave to the system resolver).

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-3.md § N-Info-2`
- Task 062 (the F2 fix this hardens).
- Can bundle with task 063 (both touch the same F2 branch).

Effort estimate: 15 min including the test.

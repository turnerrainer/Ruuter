# 052 — Re-apply SSRF check on HTTP redirects (h2ck.me S6)

## Filed

2026-07-20 — surfaced by h2ck.me pre-publication audit (finding S6,
`REVIEW.md`), still open after the 2026-07-19 fix batch (see
`POST-FIX-REVIEW.md`). Pinned by
`tests/security_hardening.rs::ssrf_check_does_not_reapply_after_redirect`.

## Severity

**High** — cloud-metadata theft via allowlisted upstream that 302s
to a blocked origin. The metadata endpoint is unauthenticated; a
successful pivot returns short-lived STS credentials.

## Problem

`HttpClient::new` / `HttpClient::with_timeout_ms` at
`src/http_client/mod.rs:161-236` build the reqwest client via

```rust
let client = Client::builder()
    .timeout(default_timeout)
    .build()
    .unwrap();
```

with no `.redirect(...)` call. reqwest's default policy is
`Policy::limited(10)` — the client transparently follows up to ten
3xx hops. `check_ssrf` at `:244-287` runs once per DSL-visible
`http.<verb>` call. So:

1. DSL calls `http://api.example.com/reports` (allowlisted).
2. Upstream responds `302 Location: http://169.254.169.254/latest/meta-data/iam/security-credentials/`.
3. reqwest follows without asking `check_ssrf` again.
4. DSL gets the STS credentials back in `${r.response.body}`.

Same holds for chained 302s to any RFC 1918, `127.0.0.0/8`, or the
loopback listener itself.

## Fix

Option 1 (preferred, from `REMEDIATION.md § S6`) — disable
transparent follow. DSL authors handle 3xx themselves via a
follow-up `http.<verb>` step (which goes through `check_ssrf`).

```rust
// src/http_client/mod.rs — both HttpClient::new and with_timeout_ms
let client = Client::builder()
    .timeout(default_timeout)
    .redirect(reqwest::redirect::Policy::none())
    .build()
    .unwrap();
```

Effort: 15 min including CHANGELOG note. Two constructor sites
(`fn new`, `fn with_timeout_ms`) need the same one-line change.

Option 2 (preserves current DSL contract) —
`Policy::custom(|attempt| ...)` closure that re-runs `check_ssrf`
on each hop. Rejected in the review because it couples security
logic to a reqwest callback where a future reqwest upgrade could
change closure semantics silently.

## Acceptance

- Both `Client::builder()` sites configure
  `.redirect(reqwest::redirect::Policy::none())`.
- Flip the assertion in
  `tests/security_hardening.rs::ssrf_check_does_not_reapply_after_redirect`
  — the blocked origin must NOT be reached, and the DSL should
  receive `status: 302` with `headers.location` set.
- Add a positive test: a DSL that reads `${r.response.status}`
  and issues a follow-up `http.get` to the Location succeeds (and
  the follow-up goes through `check_ssrf` — pin this with an
  allowlist that permits the target and a blocklist that denies
  the redirect target, confirming the second hop is checked).
- CHANGELOG entry under `### Breaking` describing the DSL-level
  migration.

## Non-goals

- Following the redirect internally with per-hop SSRF is out of
  scope (Option 2 rejected).
- Automatic re-execution of a DSL step against the new Location.

## Cross-reference

- `projects/Ruuter-on-Rust/REVIEW.md § S6` (h2ck.me workspace)
- `projects/Ruuter-on-Rust/REMEDIATION.md § S6`
- `projects/Ruuter-on-Rust/POST-FIX-REVIEW.md`
- Related: task 057 (N1 — path-scoped substring, same file);
  bundle in one PR.

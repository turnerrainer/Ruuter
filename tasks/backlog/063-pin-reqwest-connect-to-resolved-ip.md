# 063 — Pin reqwest outbound connect to the pre-resolved IP (F2 addendum, DNS-rebinding TOCTOU)

## Filed

2026-07-20 — surfaced by h2ck.me third-round follow-up
(`POST-FIX-REVIEW-3.md` § N-Info-1). Documented caveat of the
F2 fix (task 062).

## Severity

**Low** — DNS-rebinding TOCTOU on the hostname blocklist path. Not
exploitable via a stock recursive resolver: recursive DNS caches
the answer, so the flip between `check_ssrf`'s `lookup_host` call
and reqwest's own connect-time resolve requires an
attacker-controlled resolver in front of the deployment (or an
attacker who controls the authoritative zone AND a very low TTL AND
persuades the deployment's resolver to skip cache). Realistic
exposure is small; the fix is defense-in-depth.

## Problem

`src/http_client/mod.rs::check_ssrf` (the F2 branch, roughly
`:320-370`) resolves the URL hostname via `tokio::net::lookup_host`
and rejects when any returned address hits `is_private_or_local`.
reqwest then dispatches the request — but reqwest resolves the
hostname AGAIN when it actually connects, via its own DNS resolver
inside `hyper-util`. If a DNS-rebinding attacker returns different
addresses to the two queries — one public (passes the check), one
private (used for the connect) — the blocklist is bypassed.

The fix comment inside `check_ssrf` acknowledges this and defers
the pin to a follow-up task.

## Fix

Two viable approaches:

### Option A — `ClientBuilder::resolve()` per-request

reqwest exposes `ClientBuilder::resolve(hostname, SocketAddr)` and
`ClientBuilder::resolve_to_addrs(hostname, &[SocketAddr])`. These
are **client-wide** static mappings, which doesn't fit a per-request
resolution model without cloning the client per request (expensive).

Alternative: build a fresh client for each private-network-relevant
outbound with the resolved address pinned. Adds allocation and
loses reqwest's connection pool for that call.

### Option B — custom hyper `Connect` service

Build a `hyper_util::client::legacy::Client` with a custom
`Connect` service that reuses the pre-resolved `SocketAddr` from
`check_ssrf` (thread the address through as a request extension
or via a per-request client). This is the "correct" defence but is
a larger refactor because Ruuter's outbound path is reqwest, not
raw hyper.

### Recommendation

Implement Option B on the existing UDS pool pattern (`uds_pool.rs`
already speaks hyper directly). Move the private-network-hostname
path off reqwest and onto a small hyper-based dispatcher that
consumes the pre-checked `SocketAddr`. Keep reqwest for IP-literal
targets and for hostnames that resolve to public addresses only
(where the rebinding attack has no leverage).

## Acceptance

- A new test in `tests/security_new_probes_*.rs`:
  - Set up a fake DNS resolver (custom `tokio` executor with a
    controlled `lookup_host` shim) that returns a PUBLIC IP on the
    first call and a PRIVATE IP on the second.
  - Assert the outbound request is rejected — either by check or
    by connect-time refusal.
- The existing F2 negative
  (`break_n4_hostname_to_private_ip_bypasses_blocklist`) must
  remain PASSED (localhost → 127.0.0.1 still blocked).
- The F2 positive
  (`f2_positive_allowlisted_hostname_passes_even_with_blocklist_on`)
  must remain PASSED (explicit allowlist still opts in).

## Non-goals

- Pinning per-connection in the reqwest connection pool (reqwest
  already resolves fresh per connection; that's not the attack
  vector — the attack is between our check and reqwest's connect).
- Blocking all hostname outbounds (that's the "reject non-IP hosts"
  stop-gap from the F2 task; keeping hostname outbounds working is
  the point of this task).

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-3.md § N-Info-1`
- Task 062 (the F2 fix this hardens).

Effort estimate: 3-4 hours if Option B is taken; 30 min for a
narrower Option A per-call rebuild. Recommend Option B for a proper
release cut; Option A is a viable interim.

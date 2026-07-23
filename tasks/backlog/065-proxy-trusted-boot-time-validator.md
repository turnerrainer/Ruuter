# 065 — Validate `proxy.trusted` at boot, WARN on unparseable entries

## Filed

2026-07-20 — surfaced by h2ck.me third-round follow-up
(`POST-FIX-REVIEW-3.md` § N-Info-4). Documented behaviour of the
N3 fix (task 059); pinned by
`tests/security_new_probes_2.rs::n3_unparseable_trusted_entries_silently_dropped`.

## Severity

**Low** — config-validation footgun. Safe failure mode (no trust
granted), but silent: an operator who typos `trusted: ["127.0.0.1/8"]`
(intending CIDR) or `192.168.1.999` (out-of-range octet) sees their
XFF adoption silently disabled. Audit-log origins revert to peer
IPs and the operator has no boot-time signal that their config was
wrong.

## Problem

`src/router/mod.rs::resolve_origin` uses
`.filter_map(|t| t.parse::<IpAddr>().ok().map(canonicalise_ip))`
to build the effective trust list per request. Entries that don't
parse as `IpAddr` are dropped, always, no warning.

## Fix

Two options, not mutually exclusive:

### Option A — boot-time WARN log

Walk `config.proxy.trusted` in `AppConfig::load_or_default` (or in
`main.rs` right after config load); for each entry that fails
`IpAddr::from_str`, emit a WARN with the offending value and a
hint about the expected format:

```rust
for entry in &config.proxy.trusted {
    if entry.parse::<std::net::IpAddr>().is_err() {
        tracing::warn!(
            entry = %entry,
            "proxy.trusted entry does not parse as IpAddr — will be ignored. \
             Expected forms: `127.0.0.1`, `10.0.0.1`, `::1`, `2001:db8::1`. \
             CIDR ranges are not supported."
        );
    }
}
```

### Option B — strict-mode boot refusal

Optional `proxy.strict_trusted: bool` (default false to preserve
compatibility). When true, ANY unparseable entry fails config load
with a clear error, refusing to boot. Recommended for
production-deploy checks; leave off in dev so half-configured
setups don't fail-closed unexpectedly.

## Acceptance

- Ship Option A at minimum. Add a test that captures the WARN log
  (`tracing-test` or the existing test harness's log capture, if
  any) and asserts an "ignored" WARN is emitted for each of
  `127.0.0.1/8`, `not-an-ip`, `192.168.1.999`.
- If Option B lands: add positive test (`strict_trusted: true` +
  clean list boots), negative test (`strict_trusted: true` + one
  bad entry returns a config error before axum starts).
- Existing pin
  `n3_unparseable_trusted_entries_silently_dropped` still passes
  under Option A (behaviour unchanged — just noisier logs) or gets
  updated / removed under Option B.

## Non-goals

- CIDR support in `proxy.trusted` — separate task. Requires an IP
  range library and a per-request range check; today's exact-match
  semantics are simpler and match how ops normally configure
  L4 proxies.

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-3.md § N-Info-4`
- Task 059 (the N3 fix this hardens).

Effort estimate: 20 min for Option A + test; 45 min for Option A+B
combined.

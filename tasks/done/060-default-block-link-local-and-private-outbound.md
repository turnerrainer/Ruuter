# 060 — Default-deny outbound to link-local, loopback, and RFC1918 (h2ck.me N4, design pin)

## Filed

2026-07-20 — surfaced by h2ck.me post-fix follow-up sweep
(finding N4 — design pin, `POST-FIX-REVIEW.md`). Pinned by
`tests/security_new_probes.rs::default_config_permits_link_local_metadata_target`.

## Severity

**Medium** if adopted as a fix; **design decision** if we stay on
the current model. With the default config
(`InternalRequestsConfig::default()` — empty `allowed_urls`, empty
`allowed_ips`, `disabled: false`), a DSL can call any URL — in
particular:

- `http://169.254.169.254/...` — AWS/GCP/Azure metadata endpoint.
- `http://127.0.0.1:...` — sibling services on the same host.
- `http://10.*`, `http://172.16.*`, `http://192.168.*` — private
  networks the framework may be inside.
- `http://[::1]/`, `http://[fe80::.../` — IPv6 loopback and
  link-local.

Cloud-metadata SSRF is one of the two "textbook" SSRF outcomes
(the other being internal service scanning). Neither is closed
by default.

## Problem

`src/config/mod.rs:280-289`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct InternalRequestsConfig {
    #[serde(default)]
    pub disabled: bool,

    #[serde(default)]
    pub allowed_ips: Vec<String>,

    #[serde(default)]
    pub allowed_urls: Vec<String>,
}
```

Default = fully permissive. Operator must set `allowed_ips` or
`allowed_urls` to close cloud-metadata SSRF. Documentation exists
but operators miss it.

The task-044 self-call short-circuit (`http.<verb>` to our own
listener via loopback) is why we can't just outright block
`127.0.0.0/8`.

## Options

### Option A — default blocklist, opt-in override

Ship a default blocklist of link-local + loopback + RFC1918 that
runs in `check_ssrf`. Add a new opt-out for operators whose
integration model relies on private targets:

```toml
[internal_requests]
allow_private_targets = false   # default; set true for local integrations
# or list-based:
private_target_exceptions = ["10.0.0.0/8"]
```

Loopback stays reachable when the self-call short-circuit fires
(that path bypasses `check_ssrf` today; keep that). Explicit
`allowed_ips` / `allowed_urls` entries also override the blocklist.

Blocklist (IPv4): `169.254.0.0/16`, `127.0.0.0/8`, `10.0.0.0/8`,
`172.16.0.0/12`, `192.168.0.0/16`. IPv6: `::1`, `fc00::/7`,
`fe80::/10`, `::ffff:0:0/96` (v4-mapped — canonicalise before
compare).

### Option B — boot-time warning only

Emit a `WARN` at startup when the framework is bound to a
non-loopback interface AND `allowed_ips` / `allowed_urls` are
both empty AND `disabled` is false. Keep the default permissive
posture; make the risk visible.

### Option C — documentation only

Update `book/src/framework/inter-service-transport.md` with a
prominent security section: "You MUST set `allowed_ips` for any
internet-exposed deployment." No code change.

## Recommendation

**Option A** aligns with the defense-in-depth intent of the S3
fix (`internal_requests.disabled`) and matches operator
expectations for a modern router. Explicit `allowed_ips` /
`allowed_urls` still work; operators integrating with a private
network add one CIDR to `private_target_exceptions`.

If breaking-change budget is tight for the next release, ship
Option B alongside Option C now and defer Option A to a major
version bump.

## Acceptance (Option A path)

- `check_ssrf` blocks any URL whose parsed host is in the default
  blocklist, unless the operator has explicitly listed it in
  `allowed_ips` / `allowed_urls` / `private_target_exceptions`.
- Self-call short-circuit still works — because it runs BEFORE
  `check_ssrf` on the loopback path today.
- New config field `internal_requests.allow_private_targets`
  and/or `internal_requests.private_target_exceptions` land in
  `src/config/mod.rs`.
- `tests/security_new_probes.rs::default_config_permits_link_local_metadata_target`
  is flipped: victim listener MUST NOT be contacted with default
  config; the DSL sees a "blocked by default private-target
  policy" error.
- New test: with `allow_private_targets: true`, the previously
  blocked call succeeds.
- New test: with explicit `allowed_ips: ["127.0.0.1"]`, loopback
  works even with the default blocklist active.

## Non-goals

- DNS-rebinding protection (resolve host, check IP) — separate
  task if the blocklist adopts DNS-based enforcement.
- Egress firewall / L4 rules — that's the deployment platform's
  job.

## Cross-reference

- `projects/Ruuter-on-Rust/POST-FIX-REVIEW.md § N4`
- `projects/Ruuter-on-Rust/REVIEW.md § S3` (task-044 self-call
  interaction — do not regress)
- Ideally ship AFTER task 052 (redirect fix) so the blocklist
  covers redirect targets too.

Effort estimate:
- Option A: ~2 hours (config schema, blocklist, tests, docs).
- Option B: ~30 min.
- Option C: ~30 min.

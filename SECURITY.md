# Security policy

## Reporting a vulnerability

Please **do not open a public GitHub issue** for security-sensitive
findings. Instead:

1. **Preferred**: use GitHub's private vulnerability reporting for
   this repo — Security tab → **Report a vulnerability**. That routes
   the report to maintainers via a private thread with tracking.
2. **Fallback**: email `rainer.turner@gmail.com` with `[Ruuter-security]`
   in the subject line.

Include, when you can:

- Affected version (image tag or git ref)
- Reproduction steps or PoC
- Impact assessment (what an attacker gains)
- Any suggested mitigation

## Response commitments

- **Acknowledgement**: within 3 business days of the report reaching
  a maintainer.
- **Triage decision** (accepted / needs-more-info / not-a-vuln):
  within 7 business days.
- **Fix + coordinated disclosure**: target 30 days for CRITICAL and
  HIGH severity, 90 days for MEDIUM. Extension is negotiable if a
  fix requires a coordinated upstream change.
- **Credit**: reporters are credited in the release notes unless they
  ask to remain anonymous.

## Supported versions

Only the latest published release receives security fixes. Ruuter is
pre-1.0 and follows SemVer — minor bumps are the norm, patch releases
are cut only for critical fixes on the current line.

| Version   | Support status                                     |
|-----------|----------------------------------------------------|
| `0.7.x`   | ✅ Supported (current release line)                 |
| `< 0.7.0` | ❌ Not supported — upgrade to the current release  |

## What we do to reduce supply-chain risk

- **`cargo audit --deny warnings`** — every push, every PR, daily at
  06:00 UTC. Advisory exceptions live in `.cargo/audit.toml` with a
  rationale and a review date; blind ignores are a code smell.
- **`cargo deny check all`** — enforces license allow-list (Apache-2.0
  compatible only, no GPL/AGPL/SSPL), refuses git-URL deps and
  wildcard version specs, warns on duplicate crate versions. Config:
  [`deny.toml`](./deny.toml).
- **Trivy image scan** on every release-tag publish, gated on
  `HIGH` and `CRITICAL` fixed vulnerabilities. Blocks signing.
- **cosign keyless signatures** on every published image digest via
  Sigstore OIDC. Verify recipe in
  [`book/src/ops/docker.md`](./book/src/ops/docker.md#verify-the-image-cosign).
- **In-toto provenance + SPDX SBOM** attached to every multi-arch
  manifest. Inspect with `docker buildx imagetools inspect --format
  '{{ json .Provenance }}' <image>`.
- **Reproducible image layer timestamps** (`SOURCE_DATE_EPOCH` +
  `rewrite-timestamp=true`) so the same commit produces the same
  image digest. Rust binary bit-for-bit determinism is not yet
  enforced.
- **Multi-arch smoke test** — every release image is booted under
  QEMU on both `linux/amd64` and `linux/arm64` and probed with
  `/health` + `/samples/ping` before it's signed. A signed image is
  a working image.
- **Non-root container user** (uid 1000), read-only rootfs,
  `cap_drop: ALL`, `no-new-privileges: true` in the shipped
  `docker-compose.yml`.
- **Hardening surface** (v0.7.0 audit sweep) — see the
  [book chapter](./book/src/ops/security-checklist.md) and
  [`HANDOFF.md`](./HANDOFF.md).

## What is out of scope

Ruuter is a routing framework. The following are the operator's
responsibility, not Ruuter's:

- Secret fetching (Vault / KMS / Docker secrets — see
  [Constants](./book/src/dsl/constants.md#secrets))
- Persistent state / cross-replica coordination (use Resql or
  equivalent — see [State](./book/src/dsl/steps/state.md))
- IAM / JWT validation (a DSL guard can inspect headers; the
  framework does no cryptographic verification)
- Rate limiting (terminate at a reverse proxy)

See [What Ruuter does NOT do](./book/src/reference/non-goals.md) for
the full list.

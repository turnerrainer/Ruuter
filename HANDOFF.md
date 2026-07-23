# HANDOFF

**Written**: 2026-07-24
**Last verified green**: 2026-07-24 (three consecutive audit runs; 205 tests pass, 3 timing-sensitive ignores, `cargo audit --deny warnings` clean, clippy clean under both scripting feature sets, `dsl-test` 99/99 scenarios pass).
**Branch**: `feat/remove-idempotency-plus-security` (working tree, unmerged, targets `dev` via PR).
**Release**: v0.7.0 (Cargo.toml + CHANGELOG dated 2026-07-24).

This file is the entry point for the next contributor (human or agent). Read this, run the first-run checklist, then dive into the specific files it points at.

## First-run checklist

1. **Open a fresh shell** in `/home/rainer/Desktop/Buerostack/Ruuter-on-Rust`.
2. **Run the verification set** — this is now the release gate:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo clippy --all-targets --no-default-features --features scripting-quickjs -- -D warnings
   cargo test --no-fail-fast
   cargo audit --deny warnings
   ./target/debug/dsl-lint --dsl DSL/samples --constants constants.ini
   ./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
   ( cd book && mdbook build )
   ```
   Baseline: fmt clean, clippy clean (both feature sets), 205/0/3 (pass/fail/ignored), 0 vuln + 0 audit warn, 61 DSL files 0 errors, 99/99 DSL scenarios pass, mdBook builds.

## Release state at handoff

- **Cargo.toml `version = "0.7.0"`** and CHANGELOG `[0.7.0] - 2026-07-24`. This IS the release. The prior "no releases between 0.6.6 and 1.0.0" rule was superseded on 2026-07-24; publishing v0.7.0 as a stable tag today.
- **Directory renamed** on 2026-07-21: `Buerostack/Ruuter on Rust` → `Buerostack/Ruuter-on-Rust`. Purpose: kill the space in the path (broke shell tools, container mounts, and LSPs that assume `[a-zA-Z0-9_-]+`).

## What just landed (v0.7.0)

Full detail in `CHANGELOG.md`. High-level:

**Original v0.7.0 batch (2026-07-20):**

- **Idempotency-Key feature removed** (S1 / S5). DSL authors implement idempotency via `state.set` with their own body-hash + identity keys. See `book/src/dsl/idempotency-pattern.md`.
- **SSRF hardening** (S2 / S3 / S3-ext / S6 / N1 / N4 / F1 / F2). `check_ssrf` in `src/http_client/mod.rs` is the authoritative gate: exact-origin allowlist, path-segment boundary, `internal_requests.disabled` top-level gate honoured by every transport including UDS, `redirect(Policy::none())` on the reqwest client, default `block_private_networks: true` blocklist that resolves hostnames via `tokio::net::lookup_host` and rejects on any private hit.
- **`X-Forwarded-For` trust model** (S4 / N2 / N3). `proxy.trusted: [ip, …]` config. XFF/X-Real-IP only promoted to `incoming.origin` when the direct TCP peer is in that list. Leftmost-IP-only, IPv4-mapped-IPv6 canonicalised.
- **`/health` slimmed** (S7). Returns `{"status":"ok"}` only.
- **Supply chain** (S8). `serde_yml → serde_yaml_ng 0.10`. `boa_engine 0.19 → 0.20` (drops `fast-float 0.2.0` SIGSEGV). `anyhow → 1.0.104`.
- **CI audit gate** (task 056). `.github/workflows/security.yml` runs `cargo audit --deny warnings` on push / PR / daily 06:00 UTC cron. Exceptions in `.cargo/audit.toml` with review-2026-10-01 dates: RUSTSEC-2024-0384 (`instant`), RUSTSEC-2024-0436 (`paste`) — both transitive-only.

**Release audit sweep (2026-07-24):**

- **`state.delete` accepts `remove:` alias** via `#[serde(alias = "remove")]` on `StateOp::Delete`. End-to-end verified through the loader parse path (tests in `src/steps/state.rs`).
- **`dsl-test` mock modes** now build the test harness with `block_private_networks=false` (mock server binds on 127.0.0.1; production `check_ssrf` behaviour unchanged). Fix in `src/bin/dsl_test.rs::mock_test_config` + new `src/testkit/harness.rs::build_with_config`.
- **`cargo fmt`** applied repo-wide. `[lints.clippy]` posture in Cargo.toml promoted `-D warnings` to a hard gate with a small, documented allowlist for test-fixture patterns (`field_reassign_with_default`, `doc_overindented_list_items`, `doc_list_item_no_indent`, `doc_lazy_continuation`).
- **Docs synced to v0.7.0.** Five `v1.0.0` references corrected (`book/src/framework/tracing.md`, `book/src/framework/self-call-optimization.md`, `book/src/framework/pipeline.md`, `book/src/reference/non-goals.md`, `book/src/reference/changelog.md`). Also inline `pre-v1.0` comments in `src/config/mod.rs`, `src/http_client/mod.rs`, `src/router/mod.rs`.
- **`DSL/samples/POST/idempotent-transfer.yml`** header rewritten to describe the DSL-authored pattern (the removed framework feature was still described in the old header).

## Documentation & DSL coverage

- 11 documented DSL step chapters (`book/src/dsl/steps/*.md`) match 1:1 with variants in `src/steps/mod.rs::DslStep`.
- 61 sample DSLs under `DSL/samples/` cover all 11 step primitives.
- 61 test DSLs under `DSL-tests/samples/` cover every sample except the 5 `bench-*.yml` files (intentional — benchmarking targets, not user-facing).
- `book/src/framework/ssrf.md` and `book/src/dsl/idempotency-pattern.md` match the current implementation.

## Open backlog

Files under `tasks/backlog/`:

| Task | Origin | What |
|---|---|---|
| 017 | Older | Externalize stateful streams |
| 026 | Older | Redeploy container |
| 040 | Older | Parallel HTTP with concurrency cap |
| 041 | Older | First-N-succeed fan-out aggregation |
| 063 | F2 addendum | Pin reqwest connect to pre-resolved IP (DNS-rebinding TOCTOU) |
| 064 | F2 addendum | Wrap `tokio::net::lookup_host` in timeout |
| 065 | N-Info-4 | WARN on unparseable `proxy.trusted` entries at boot |
| 066 | Downstream ARM support (2026-07-22) | Multi-arch Docker image (linux/amd64 + linux/arm64) |
| 067 | DSL hygiene (2026-07-20) | Add `#{const}` alternate syntax for constant interpolation |
| 068 | 2026-07-24 audit | Address rquickjs-core 0.6.2 future-incompat warning |

Closed in v0.7.0 and moved to `tasks/done/`: 052, 053, 054, 055, 057, 058, 059, 060, plus older 056, 061, 062.

## External context

- **h2ck.me review workspace** lives at `/home/rainer/Desktop/h2ck.me/projects/Ruuter-on-Rust/`. Not affiliated with the upstream repo, not committed back. Files: `README.md`, `REVIEW.md`, `POST-FIX-REVIEW.md`, `POST-FIX-REVIEW-2.md`, `POST-FIX-REVIEW-3.md`, `REMEDIATION.md`, `TEST-SUITE.md`, plus `research/`.
- **Auto-memory folder** at `/home/rainer/.claude/projects/-home-rainer-Desktop-Buerostack-Ruuter-on-Rust/memory/` — the "no releases between v0.6.6 and v1.0.0" rule needs updating (v0.7.0 shipped 2026-07-24).
- **User's global config** at `/home/rainer/.claude/CLAUDE.md` — sequential-work rules (no parallel dependent operations, max 5-10 concurrent tool calls) and the audit-cycle lessons block.

## Design decisions worth remembering

Consolidated from the last few conversations:

- **Cross-replica state consistency: Resql first, pub/sub only under latency pressure.** For cron-frequency writes, a Resql lookup per read is 1-5ms typical and eliminates the "N pods diverge" class of bug. Skip pub/sub for CronManager-scheduled writes.
- **`state.*` is a per-process cache, not a KV store.** No TTL, no eviction, no persistence. Container restart wipes everything. Anything durable belongs in Resql.
- **DSL YAML samples in book/ must be pure block style.** No flow-style `{ … }` maps — a reader should be able to copy any snippet straight into a DSL file.
- **N4 default is `block_private_networks: true`.** Any test / fixture that binds a listener on 127.0.0.1 needs `block_private_networks: false` in its `InternalRequestsConfig`, OR use `Harness::build_with_config` in dsl-test (mock modes handle this automatically).
- **DNS-rebinding TOCTOU is documented, not fixed.** `check_ssrf` resolves hostnames but reqwest re-resolves on connect. Comment in `src/http_client/mod.rs` points at task 063.
- **Clippy `-D warnings` is a hard CI gate.** `[lints.clippy]` in Cargo.toml carries the small allowlist. Two mutually-exclusive scripting features (`scripting-boa`, `scripting-quickjs`) mean `--all-features` doesn't work; check each set separately.

## Where to look for more detail

| Topic | File |
|---|---|
| Full CHANGELOG for v0.7.0 | `CHANGELOG.md` (top of file, `[Unreleased]` + `[0.7.0]`) |
| SSRF match rules + blocklist | `book/src/framework/ssrf.md` |
| `state` step semantics + samples | `book/src/dsl/steps/state.md` |
| Idempotency (DSL-side) | `book/src/dsl/idempotency-pattern.md` |
| CI security gate config | `.github/workflows/security.yml`, `.cargo/audit.toml` |
| Test map | `tests/security.rs`, `tests/security_hardening.rs`, `tests/security_new_probes*.rs` |
| h2ck.me review threads | `/home/rainer/Desktop/h2ck.me/projects/Ruuter-on-Rust/POST-FIX-REVIEW-3.md` (final roll-up) |
| Follow-up task briefs | `tasks/backlog/063-*.md`, `064-*.md`, `065-*.md`, `066-*.md`, `067-*.md`, `068-*.md` |

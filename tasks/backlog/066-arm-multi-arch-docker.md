# 066 — Multi-arch Docker image (linux/amd64 + linux/arm64)

## Filed

2026-07-22 — surfaced by a downstream question about whether Ruuter
supports ARM. The Java Ruuter has always run on ARM (JVM handles
it); the Rust reimplementation currently ships an amd64-only image.

## Severity

**Low** — no security implication and no correctness bug. This is
about deployment breadth. Not blocking release.

## Motivation

- **Java Ruuter** ran on ARM out of the box; a downstream consumer
  who ports to the Rust build shouldn't lose that.
- **AWS Graviton / Azure Ampere / Apple Silicon dev laptops** are
  all common ARM hosts. Any team that treats "supports ARM" as a
  procurement checkbox will look at the Docker Hub tags first.
- **Cost**: Graviton is ~20% cheaper than equivalent x86 EC2, and
  the arm64 GitHub Actions runners are free for public repos.

## Current state

- **Source code**: portable. Pure Rust, no arch-specific asm, no
  platform ifdefs. `cargo build --target=aarch64-unknown-linux-gnu`
  works today for anyone who tries it locally.
- **Dependencies**: ARM-clean. `boa_engine`, `rquickjs` (bundled
  QuickJS C), `sha2` (with NEON), `openssl-sys` (via reqwest's
  `native-tls`) — all build for aarch64 with a standard aarch64
  toolchain in the build image.
- **`Dockerfile`**: single-stage FROM `rust:1.88-slim` + `debian:bookworm-slim`
  runtime. Both base images are already multi-arch on Docker Hub,
  so a `buildx --platform` invocation just picks the right layer
  for each target.
- **CI**: `.github/workflows/tests.yml` has two `runs-on: ubuntu-latest`
  jobs (Boa + QuickJS). Neither tests on arm64.
- **No release workflow exists** that pushes images to a registry.

## Fix

Two independent pieces; ship either or both.

### Part A — multi-arch image build (must-have)

Add a `.github/workflows/docker.yml` (or a release step in an
existing workflow) that runs `docker buildx build` for
`linux/amd64,linux/arm64` and pushes the multi-arch manifest.
Sketch:

```yaml
name: docker
on:
  push:
    tags: ["v*.*.*"]
  workflow_dispatch:
jobs:
  publish:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-qemu-action@v3
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: |
            ghcr.io/turnerrainer/ruuter-on-rust:${{ github.ref_name }}
            ghcr.io/turnerrainer/ruuter-on-rust:latest
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

QEMU emulation makes the arm64 layer slow to build (roughly 3–5×
amd64 time on a hosted amd64 runner). For faster builds, split the
job so amd64 runs on `ubuntu-latest` and arm64 runs on
`ubuntu-24.04-arm` (GA on Actions since 2025-04) natively, then
merge the manifests with `docker buildx imagetools create`.

### Part B — ARM in the test matrix (nice-to-have)

Add an arm64 job to `.github/workflows/tests.yml` so `cargo test`
runs natively on ARM. Catches endianness / alignment quirks in the
raw-TCP fixtures under `tests/security_hardening.rs` and
`tests/security_new_probes*.rs` (unlikely bugs, but the tests
themselves make wire-level assertions where an arch mismatch
would surface).

```yaml
boa-arm:
  runs-on: ubuntu-24.04-arm
  # ...same steps as `boa`, no matrix strategy needed.
```

Same for `quickjs-arm`. Cache-key namespace by arch to avoid
cross-arch pollution.

## Acceptance

- `docker manifest inspect ghcr.io/turnerrainer/ruuter-on-rust:latest`
  reports both `linux/amd64` and `linux/arm64` entries.
- `docker run --platform linux/arm64 ghcr.io/turnerrainer/ruuter-on-rust:latest --help`
  works on an ARM host (or on amd64 with `docker run --platform`).
- If Part B lands: `cargo test --release --no-fail-fast` passes on
  `ubuntu-24.04-arm`.
- CHANGELOG entry under `[Unreleased]` mentioning multi-arch image
  availability.

## Non-goals

- Adding architecture-specific perf optimisations. Rust and the deps
  handle NEON / SSE / etc. via `#[cfg(target_arch)]` internally.
- Changing base images. `rust:1.88-slim` and `debian:bookworm-slim`
  are already multi-arch.
- Windows / macOS binaries. Separate task if there's demand.
- Distroless / static-linked variants. Also separate.
- Testing on non-Linux ARM (macOS-arm64 is a dev-time target;
  handled by `cargo build` on a Mac).

## Verification once landed

- Pull image on an arm64 host, exercise the DSL-lint and dsl-test
  binaries in the image against the shipped `DSL/` tree.
- Sanity-check TLS works — reqwest's `native-tls` pulls OpenSSL,
  and the runtime `libssl3` package in the Debian slim image needs
  to be present on the arm64 layer (it is, but confirm).
- Check startup memory / CPU on Graviton3 vs. an equivalent x86
  instance; document any surprises in
  `book/src/ops/failure-modes.md`.

## Cross-reference

- `HANDOFF.md § What just landed` (points at this task as the
  ARM-support answer).
- `.github/workflows/tests.yml` (target for Part B).
- `Dockerfile` (multi-arch build target).

Effort estimate:

- Part A alone (multi-arch image, QEMU-emulated arm64 build):
  ~4 hours including CHANGELOG + docs.
- Part A with native arm64 runner (faster CI): +1 hour.
- Part B (ARM test matrix): +2 hours.

Total for full "tested and shipped on both arches": one working day.

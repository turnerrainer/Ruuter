# 053 — Minimise `/health` response; move version to admin surface (h2ck.me S7)

## Filed

2026-07-20 — surfaced by h2ck.me pre-publication audit (finding S7,
`REVIEW.md`), still open after the 2026-07-19 fix batch. Pinned by
`tests/security_hardening.rs::health_endpoint_leaks_framework_version`.

## Severity

**Low** — fingerprinting surface. Combined with an outstanding
advisory in `Cargo.lock` (task 054 / 055) it becomes an
enumeration multiplier: a scanner sweeps the internet, filters on
`service: "ruuter-on-rust"` + a vulnerable `version`, and gets a
target list.

## Problem

`src/router/mod.rs:286-292`:

```rust
async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "ruuter-on-rust",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
```

The route is unauthenticated and typically the first thing external
load-balancers hit. Anyone with network reach learns the framework
name and exact semver.

## Fix

1. Reduce `/health` to the minimum a load-balancer needs:

   ```rust
   async fn health_check() -> impl IntoResponse {
       Json(json!({ "status": "ok" }))
   }
   ```

2. Add a new `/_/status` handler that returns `service`, `version`,
   uptime, and any other operator-facing detail. Gate it behind the
   same admin-endpoint switch that already exists for
   `SourceSupervisor::admin_router` (see `src/openapi.rs` /
   `src/supervisor` for the pattern). Default: admin auth required
   or bound to loopback only.

3. Update `book/src/framework/` docs — mention `/health` returns
   `{"status":"ok"}` and point ops at `/_/status` for build info.

## Acceptance

- Flip `tests/security_hardening.rs::health_endpoint_leaks_framework_version`
  to assert `body == {"status":"ok"}` exactly (no `version`, no
  `service`).
- New test: `admin_status_requires_authentication_or_admin_binding`
  — a request to `/_/status` from an unauthenticated non-admin
  peer returns 401/403/404 (whichever the existing admin surfaces
  return).
- New test: `admin_status_returns_service_and_version_when_admin`
  — with admin credentials / admin-listener, response contains
  `service: "ruuter-on-rust"` + a semver `version`.

## Non-goals

- Fully removing the version from build artefacts (Cargo.toml still
  needs it). This task is about the wire-exposed surface only.

## Cross-reference

- `projects/Ruuter-on-Rust/REVIEW.md § S7`
- `projects/Ruuter-on-Rust/REMEDIATION.md § S7`

Effort estimate: 20 min for `/health` trim + admin handler wiring +
two tests.

# 001 — `HttpClient::clone` rebuilds the inner reqwest client

**Status**: BACKLOG.
**Severity**: MEDIUM (perf + correctness for any concurrent HTTP-heavy load; latency-critical for event-driven event pipelines).
**Effort**: 10 minutes.
**Filed**: 2026-06-17.

## What's wrong

`src/router/mod.rs:146-150` implements `Clone for HttpClient` by calling
`HttpClient::new(self.default_timeout.as_millis() as u64)`. That
constructor calls `reqwest::Client::builder().build()` — i.e. every
clone reinitialises the entire HTTP stack (TLS roots, connection
pool, DNS resolver).

In `DslRouter::execute_single_step` the `HttpStepExecutor` is built
fresh per step (`router/mod.rs:124`), so every HTTP step today pays
that cost. `reqwest::Client` is already designed to be cheap-cloned
(internal `Arc`); the wrapper here negates that.

## Fix

- Make `HttpClient { client: reqwest::Client, default_timeout: Duration }`
  derive `Clone`.
- Remove the manual `Clone for HttpClient` impl in `router/mod.rs`.

## Verification

- `cargo build` clean.
- Existing routing behaviour unchanged for the sample DSLs.

## Why this is generic

Pure infra fix. No service-specific code introduced.

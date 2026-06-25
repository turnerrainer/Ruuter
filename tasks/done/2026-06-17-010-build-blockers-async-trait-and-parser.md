# 010 — Repo doesn't build from a clean target dir

**Status**: IN-PROGRESS.
**Severity**: BLOCKER (any work on top of `dev` requires this fixed).
**Effort**: 15 minutes.
**Filed**: 2026-06-17.
**Discovered while**: working on #001 — the pre-existing `target/`
contained root-owned fingerprint files from a prior Docker build,
forcing `CARGO_TARGET_DIR=./target-local` for a fresh build, which
in turn surfaced two errors masked by incremental compilation.

## Errors

1. **`E0195` × 6** in every step executor (`src/steps/{assign,http,log,return_step,switch,template}.rs`):
   `lifetime parameters or bounds on method 'execute' do not match the trait declaration`.

   Root cause: `StepExecutor` (in `src/steps/mod.rs:114`) declares
   `async fn execute(...)` natively (Rust 1.75+), but every impl block
   above the executor has `#[async_trait::async_trait]`. The macro
   desugars to a different signature than the native async-in-trait
   the trait declares.

   Fix: remove `#[async_trait::async_trait]` from each impl block. The
   `async_trait` crate dep can stay in `Cargo.toml` for now (other
   future code may want it); not removing to keep the diff small.

2. **lifetime `'1` must outlive `'2`** in
   `src/dsl/parser.rs:32-36` — the closure passed to
   `regex::Regex::replace_all` returns `&str` that borrows from a
   `HashMap` (`self.constants`) AND from the regex captures, with
   conflicting lifetimes.

   Fix: return an owned `String` (or `Cow<str>`) from the closure
   instead of `&str`.

## Verification

- `CARGO_TARGET_DIR=./target-local cargo build` completes successfully.
- `target-local/` is added to `.gitignore`.

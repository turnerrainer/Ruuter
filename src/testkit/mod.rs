//! Test-kit primitives shared by `dsl-lint` and `dsl-test`.
//!
//! - `schema` — serde types for `.test.yml`
//! - `matcher` — JSON matchers (`deep_equal`, `subset`, wildcards)
//! - `mock_http` — tiny axum mock upstream so DSLs calling
//!   `http.get/post/...` in test mode hit a controllable, in-process
//!   server instead of the internet
//! - `harness` — helpers around DslLoader / DslRouter / TriggerDispatcher
//!   for the three test modes (inprocess, ws-client, trigger-inject)

pub mod harness;
pub mod matcher;
pub mod mock_http;
pub mod schema;

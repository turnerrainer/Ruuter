//! Serde types for `.test.yml` files.
//!
//! A test file describes one or more scenarios against a single DSL
//! (or trigger, or WS handler). File layout mirrors `DSL/`:
//!
//! ```text
//! DSL/samples/GET/basic/hello.yml       ← the DSL
//! DSL-tests/samples/GET/basic/hello.test.yml   ← its tests
//! ```
//!
//! Mode dispatch:
//!
//! - `inprocess` (default) — HTTP request through `DslRouter::execute_dsl`
//! - `mock-http` — same as inprocess but boots a mock upstream first
//!   and lets each test declare `mocks:` + `constants:` overrides
//! - `ws-client` — real WS client against an in-process axum server
//! - `trigger-inject` — synthetic frame through `TriggerDispatcher`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

fn default_mode() -> Mode {
    Mode::Inprocess
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Inprocess,
    MockHttp,
    WsClient,
    TriggerInject,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestFile {
    #[serde(default = "default_mode")]
    pub mode: Mode,

    /// Per-file constant overrides. Merged on top of the loader's
    /// `constants.ini` before the DSL tree is parsed.
    #[serde(default)]
    pub constants: HashMap<String, String>,

    /// Per-file URL rewrites for outbound HTTP calls. Any URL whose
    /// origin matches a key is rewritten to the corresponding value.
    /// The literal string `{MOCK}` in a value resolves to the mock
    /// upstream's base URL at runtime.
    ///
    /// Example:
    ///   http_rewrite:
    ///     "https://jsonplaceholder.typicode.com": "{MOCK}"
    #[serde(default)]
    pub http_rewrite: HashMap<String, String>,

    pub tests: Vec<Scenario>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scenario {
    pub name: String,

    /// Optional description shown on failure.
    #[serde(default)]
    pub description: Option<String>,

    /// HTTP request (inprocess / mock-http modes).
    #[serde(default)]
    pub request: Option<HttpRequest>,

    /// WS connect + frames (ws-client mode).
    #[serde(default)]
    pub ws: Option<WsScenario>,

    /// Trigger dispatch (trigger-inject mode).
    #[serde(default)]
    pub trigger: Option<TriggerScenario>,

    /// HTTP response expectations (inprocess / mock-http).
    #[serde(default)]
    pub expect: Option<ExpectHttp>,

    /// Setup performed before the scenario runs.
    #[serde(default)]
    pub setup: Option<Setup>,

    /// State-store assertions performed after the scenario runs.
    #[serde(default)]
    pub verify_state: Vec<StateAssertion>,

    /// Mock-upstream assertions performed after the scenario runs.
    #[serde(default)]
    pub verify_mocks: Vec<MockAssertion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub query: HashMap<String, Value>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<Value>,
    /// Origin the router should attribute the request to. Defaults to
    /// `test`.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsScenario {
    /// Endpoint path (e.g. `/samples/echo`). Project is the first
    /// segment.
    pub path: String,
    #[serde(default)]
    pub query: HashMap<String, String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Frames to send. Each becomes one WS Text frame.
    #[serde(default)]
    pub send: Vec<Value>,
    /// Frames expected back. Order-sensitive. Wildcards allowed via
    /// matcher rules (see `matcher` module).
    #[serde(default)]
    pub expect_frames: Vec<Value>,
    /// Optional: how long to wait for `expect_frames.len()` frames
    /// before failing. Defaults to 2000ms.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TriggerScenario {
    pub project: String,
    pub channel: String,
    pub key: String,
    #[serde(default)]
    pub payload: Value,
    /// Set to `false` when the test expects no matching trigger DSL
    /// (dispatcher returns `Ok(false)`). Defaults to `true`.
    #[serde(default = "default_true")]
    pub expect_dispatched: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ExpectHttp {
    #[serde(default)]
    pub status: Option<u16>,
    /// Deep-equal on the response body.
    #[serde(default)]
    pub body: Option<Value>,
    /// Subset-match on the response body. See `matcher::subset_matches`.
    #[serde(default)]
    pub body_matches: Option<Value>,
    /// Every listed header must be present with the given value.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Every listed header must exist (any value).
    #[serde(default)]
    pub header_present: Vec<String>,
    /// Every listed header must be absent.
    #[serde(default)]
    pub header_absent: Vec<String>,
    /// When true, the response must carry `Idempotency-Replayed: true`.
    /// When false, that header must be absent.
    #[serde(default)]
    pub replayed: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Setup {
    /// State-store rows to insert before the scenario runs.
    #[serde(default)]
    pub state: Vec<StateSeed>,
    /// Mock-upstream expectations. Populated only in `mock-http` mode.
    #[serde(default)]
    pub mocks: Vec<MockUpstream>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateSeed {
    pub project: String,
    pub key: String,
    pub value: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateAssertion {
    pub project: String,
    pub key: String,
    /// Expected value. Subset-match on objects, deep-equal on scalars,
    /// exact-match on arrays. Use `null` to assert the key is missing.
    pub value: Value,
}

/// One mock upstream. The mock server matches on URL substring +
/// method and returns the configured response. Any request the DSL
/// makes that doesn't match a registered mock gets 599 back so the
/// test fails loudly rather than silently.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MockUpstream {
    /// URL substring the DSL will call.
    pub url_matches: String,
    #[serde(default = "default_get")]
    pub method: String,
    #[serde(default = "default_200")]
    pub status: u16,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_get() -> String {
    "GET".to_string()
}
fn default_200() -> u16 {
    200
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MockAssertion {
    pub url_matches: String,
    #[serde(default = "default_1")]
    pub count: usize,
    /// Optional subset-match on the request body the DSL sent.
    #[serde(default)]
    pub body_matches: Option<Value>,
}

fn default_1() -> usize {
    1
}

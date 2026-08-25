use crate::context::ExecutionContext;
use crate::dsl::DeclarationStep;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod assign;
pub mod engine;
pub mod http;
pub mod http_mock;
pub mod iterate;
pub mod log;
pub mod return_step;
pub mod single_flight;
pub mod state;
pub mod switch;
pub mod template;
pub mod ws_send;

/// Java-Ruuter base step fields shared by every non-Declaration
/// step. Every executor consults these via [`DslStep::base()`] and
/// the engine wraps step dispatch with the corresponding behaviour:
///
/// - `skip: true` — engine skips the step's action, still counts as
///   a transition, falls through to the next step in source order.
/// - `sleep: <ms>` — engine sleeps that many milliseconds BEFORE
///   dispatching the step's action.
/// - `max_recursions` — per-step cap; the engine takes the min of
///   this and the global `max_step_recursions`. On exhaustion the
///   engine advances PAST the looping step rather than terminating.
/// - `reload_dsl: true` (alias `reload_dsls`, `reloadDsl`,
///   `reloadDsls`) — after the step's action runs, the engine
///   triggers a fresh DSL tree load (only if
///   `dsl.allow_dsl_reloading` is on).
///
/// All fields serde-default so a step without any of them parses
/// as `BaseStepFields::default()`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BaseStepFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<bool>,
    /// Milliseconds to sleep before executing the step's action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep: Option<u64>,
    /// Per-step recursion cap. Engine enforces min(step, global).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "maxRecursions"
    )]
    pub max_recursions: Option<u32>,
    /// Trigger a DSL tree reload after this step's action. Gated on
    /// `dsl.allow_dsl_reloading`; a step-authored request when the
    /// gate is off logs at ERROR and is otherwise a no-op.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "reload_dsls",
        alias = "reloadDsl",
        alias = "reloadDsls"
    )]
    pub reload_dsl: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DslStep {
    Assign(AssignStep),
    Return(ReturnStep),
    Http(HttpStep),
    HttpMock(HttpMockStep),
    Switch(SwitchStep),
    Log(LogStep),
    Template(TemplateStep),
    State(StateStep),
    Iterate(IterateStep),
    WsSend(WsSendStep),
    SingleFlight(SingleFlightStep),
    Declaration(DeclarationStep),
}

impl DslStep {
    /// Access the shared base fields. Returns `None` only for
    /// `Declaration`, which is DSL metadata rather than an action
    /// step and has no `skip:` / `sleep:` etc. semantics.
    pub fn base(&self) -> Option<&BaseStepFields> {
        match self {
            DslStep::Assign(s) => Some(&s.base),
            DslStep::Return(s) => Some(&s.base),
            DslStep::Http(s) => Some(&s.base),
            DslStep::HttpMock(s) => Some(&s.base),
            DslStep::Switch(s) => Some(&s.base),
            DslStep::Log(s) => Some(&s.base),
            DslStep::Template(s) => Some(&s.base),
            DslStep::State(s) => Some(&s.base),
            DslStep::Iterate(s) => Some(&s.base),
            DslStep::WsSend(s) => Some(&s.base),
            DslStep::SingleFlight(s) => Some(&s.base),
            DslStep::Declaration(d) => Some(&d.base),
        }
    }

    /// Short type name used in structured log fields (`dsl.step.type`)
    /// and OTel span names. Stable across the DSL / OpenAPI surface —
    /// dashboards that group by step type can rely on these strings.
    pub fn type_name(&self) -> &'static str {
        match self {
            DslStep::Assign(_) => "assign",
            DslStep::Return(_) => "return",
            DslStep::Http(_) => "http",
            DslStep::HttpMock(_) => "http_mock",
            DslStep::Switch(_) => "switch",
            DslStep::Log(_) => "log",
            DslStep::Template(_) => "template",
            DslStep::State(_) => "state",
            DslStep::Iterate(_) => "iterate",
            DslStep::WsSend(_) => "ws_send",
            DslStep::SingleFlight(_) => "single_flight",
            DslStep::Declaration(_) => "declaration",
        }
    }

    /// The step's explicit `next:` value, or `None` if unset.
    /// Engine treats `None` as "fall through to source-order next"
    /// (Java-parity, audit finding 03).
    pub fn explicit_next(&self) -> Option<&str> {
        match self {
            DslStep::Assign(s) => s.next.as_deref(),
            DslStep::Return(s) => s.next.as_deref(),
            DslStep::Http(s) => s.next.as_deref(),
            DslStep::HttpMock(s) => s.next.as_deref(),
            DslStep::Switch(s) => s.next.as_deref(),
            DslStep::Log(s) => s.next.as_deref(),
            DslStep::Template(s) => s.next.as_deref(),
            DslStep::State(s) => s.next.as_deref(),
            DslStep::Iterate(s) => s.next.as_deref(),
            DslStep::WsSend(s) => s.next.as_deref(),
            DslStep::SingleFlight(s) => s.next.as_deref(),
            DslStep::Declaration(_) => None,
        }
    }
}

/// Task 042 — collapse concurrent duplicate requests keyed on a
/// DSL-computed string into one execution + N wait-and-share
/// followers. Same-instance only; two Ruuter replicas each keep
/// their own map.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SingleFlightStep {
    pub single_flight: SingleFlightBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SingleFlightBody {
    /// Coalesce key. Evaluated per request; concurrent requests
    /// producing the same string collapse. Empty string is a valid
    /// (but unusual) key.
    pub key: String,
    /// Follower wait budget. If the leader hasn't published a
    /// result within `ttl_ms`, followers return a timeout error and
    /// the map slot is evicted so the next caller can lead a fresh
    /// coalesce window. Leader execution itself is NOT interrupted
    /// (its own step budgets apply); the TTL only bounds follower
    /// blocking.
    pub ttl_ms: u64,
    /// Body steps run once per coalesce window, in the leader's
    /// context. Sub-step `next:` directives are ignored — use a
    /// Return step to bail out. Same semantics as `iterate.do`.
    #[serde(rename = "do")]
    pub body: Vec<DslStep>,
    /// Name of the variable to snapshot from the leader's context
    /// after `do:` completes. That value is broadcast to followers,
    /// who bind it into THEIR context under the same name. When
    /// unset, followers still get "leader completed" (no value
    /// bound); useful for cache-warming DSLs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IterateStep {
    /// Expression that evaluates to a list. Each element is bound to
    /// the variable named by `as` and `do` runs once per element.
    pub iterate: IterateBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IterateBody {
    pub over: Value,
    #[serde(rename = "as")]
    pub item_var: String,
    #[serde(rename = "do")]
    pub body: Vec<DslStep>,
    /// Optional aggregate. If set, the value of this expression is
    /// collected (per-item) and bound into the parent context under
    /// `into`. The expression sees the iteration variable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collect: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub into: Option<String>,
    /// Cap on the number of items iterated. Defaults to 10_000 in the
    /// executor — set lower for tight bounds, higher for known-large lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateStep {
    pub state: StateOp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateOp {
    /// Read `key` from the project-scoped store into context variable `into`.
    /// Missing keys bind `null`.
    Get { key: String, into: String },
    /// Write `value` (evaluated through the script engine) under `key`.
    Set { key: String, value: Value },
    /// Remove `key`. No error if absent. Accepts both `delete:` and
    /// `remove:` as YAML keys — DSL authors coming from Java Ruuter
    /// or from a Redis/DEL background reach for either verb.
    #[serde(alias = "remove")]
    Delete { key: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssignStep {
    pub assign: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReturnStep {
    #[serde(rename = "return")]
    pub return_value: Value,
    /// HTTP status: literal u16 OR a script expression like
    /// `${upstream.response.status}` that evaluates to a u16.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, Value>>,
    /// Java-parity: default true — wrap response in `{"response": ...}`
    /// envelope unless explicitly `wrapper: false`. Handled by the
    /// router at response-serialisation time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpStep {
    pub call: String,
    pub args: HttpArgs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Java-parity `error:` step name. On upstream non-allowed
    /// status the executor jumps to this step instead of propagating
    /// the error (audit finding 04).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpArgs {
    pub url: String,
    /// Body — any JSON value. Object (map), array, or scalar all work.
    /// Use a YAML mapping for explicit fields, or `${incoming.body}` to
    /// pass the inbound body through verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SwitchStep {
    pub switch: Vec<Condition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Condition {
    pub condition: String,
    pub next: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogStep {
    pub log: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsSendStep {
    pub ws_send: WsSendArgs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsSendArgs {
    /// Connection id to send to. Evaluated as a script expression.
    /// May resolve to a string (single recipient) or an array of
    /// strings (fan-out). When omitted, the current
    /// `context.connection_id()` is used — typical pattern inside a
    /// WS server DSL replying to the originating client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
    /// JSON payload. Evaluated through the script engine, so any
    /// `${...}` expressions inside resolve against context.
    pub payload: Value,
    /// Optional broadcast filter. If set, `to` is ignored and the
    /// payload is broadcast to every connection whose id starts with
    /// this prefix. Useful for room-style fan-out (e.g. `client:`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcast_prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateStep {
    pub template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<HashMap<String, Value>>,
    /// Java-parity: header values are `Value` (any JSON, incl. maps
    /// with `${…}` expressions) and are evaluated at call time
    /// (audit finding 06). String is coerced from evaluated value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

/// Java-parity `call: reflect.mock` step. Runs no HTTP request; binds a
/// synthetic HttpStepResult under `result:` so downstream steps see
/// the same shape as an `http.<verb>` call (audit finding 09).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpMockStep {
    pub call: String,
    pub args: HttpMockArgs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpMockArgs {
    /// Optional request-shape echo. When present, is bound under
    /// `.request` on the synthetic HttpStepResult so DSLs can assert
    /// on what they "would have sent." Not evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Value>,
    /// The mocked response. Bound under `.response.body` on the
    /// synthetic HttpStepResult; `.response.status` defaults to 200.
    pub response: Value,
    /// Optional response status. Defaults to 200 when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

pub trait StepExecutor {
    fn execute(
        &self,
        context: &ExecutionContext,
    ) -> impl std::future::Future<Output = Result<StepResult>> + Send;
}

#[derive(Debug, Clone, Default)]
pub struct StepResult {
    pub next_step: Option<String>,
    pub goto_step: Option<String>,
    pub should_return: bool,
    pub return_value: Option<Value>,
    pub return_status: Option<u16>,
    pub return_headers: Option<HashMap<String, String>>,
    /// Audit finding 05/12 — carries the ReturnStep's `wrapper:`
    /// value (default `true` per Java parity). Router uses this at
    /// response serialisation to decide whether to wrap in
    /// `{"response": <value>}`.
    pub return_wrapper: Option<bool>,
}

impl StepResult {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_next(next: String) -> Self {
        Self {
            next_step: Some(next),
            ..Self::new()
        }
    }

    pub fn with_return(
        value: Value,
        status: Option<u16>,
        headers: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            should_return: true,
            return_value: Some(value),
            return_status: status,
            return_headers: headers,
            ..Self::new()
        }
    }
}

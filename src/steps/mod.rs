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
pub mod parallel_http;
pub mod return_step;
pub mod single_flight;
pub mod state;
pub mod switch;
pub mod template;
pub mod ws_send;
pub mod ws_tag;

/// Issue #82 — single source of truth for step-primitive YAML keys.
/// Consumed by two call sites that MUST stay in sync but historically
/// drifted (PR #80 fixed a two-year gap where `ws_tag:` landed in the
/// runtime parser without a matching update to the linter):
///
/// - `src/dsl/parser.rs` uses `ACTION_STEP_KEYS` (subset — excludes
///   `declaration:` because that's metadata, not an action; a step
///   listing both `declaration:` and `assign:` is valid and must not
///   trip the "one step = one action" check from issue #56).
/// - `src/bin/dsl_lint.rs` uses `STEP_KEYS` (full list — includes
///   `declaration:` because `declaration:`-only steps are legal
///   top-level DSL elements the linter must recognise).
///
/// Adding a new step primitive:
/// 1. Add the key here (both lists if it's an action, only `STEP_KEYS`
///    if it's metadata-only).
/// 2. Add the parse dispatch in `src/dsl/parser.rs::parse_step`.
/// 3. Add a variant to `DslStep` below.
/// The invariant `ACTION_STEP_KEYS = STEP_KEYS \ {"declaration"}` is
/// pinned by a unit test at the bottom of this module.
pub const STEP_KEYS: &[&str] = &[
    "assign",
    "call",
    "declaration",
    "iterate",
    "log",
    "parallel_http",
    "return",
    "single_flight",
    "state",
    "switch",
    "template",
    "ws_send",
    "ws_tag",
];

/// See `STEP_KEYS`. `declaration:` is metadata, not an action, so it
/// is intentionally absent from this subset — used by the parser's
/// "one step = one action" check (issue #56).
pub const ACTION_STEP_KEYS: &[&str] = &[
    "assign",
    "call",
    "iterate",
    "log",
    "parallel_http",
    "return",
    "single_flight",
    "state",
    "switch",
    "template",
    "ws_send",
    "ws_tag",
];

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
    /// Issues #135 + #136 — bounded fan-out to N peer gates with
    /// structured aggregation. Composes with the detach step (#137)
    /// for the eFTI K4 pattern: write results to Postgres via Resql,
    /// caller polls back.
    ParallelHttp(ParallelHttpStep),
    WsSend(WsSendStep),
    WsTag(WsTagStep),
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
            DslStep::ParallelHttp(s) => Some(&s.base),
            DslStep::WsSend(s) => Some(&s.base),
            DslStep::WsTag(s) => Some(&s.base),
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
            DslStep::ParallelHttp(_) => "parallel_http",
            DslStep::WsSend(_) => "ws_send",
            DslStep::WsTag(_) => "ws_tag",
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
            DslStep::ParallelHttp(s) => s.next.as_deref(),
            DslStep::WsSend(s) => s.next.as_deref(),
            DslStep::WsTag(s) => s.next.as_deref(),
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

/// Issues #135 + #136 — bounded fan-out to N peer gates with
/// structured aggregation. Replaces the Klite-multiplexer pattern
/// retired by K4 (kemit-ee/efti-gate-ee#252). Each peer gets its own
/// outbound HTTP call via the shared `HttpClient` (so SSRF checks,
/// pinned-DNS resolution, and the `#89` transport-error contract all
/// apply identically to each peer). The result variable binds to an
/// array of `{peer, response}` objects — one entry per peer, where
/// `response` carries `{status, body, headers, error}` in the shape
/// of `HttpResponse` (status 0 + non-null error = transport failure).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParallelHttpStep {
    pub parallel_http: ParallelHttpBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParallelHttpBody {
    /// Expression evaluating to an array of peer objects. Each element
    /// is bound to `${peer}` for the duration of the args evaluation
    /// (same binding model as `iterate.as`). Typical shape:
    /// `${gates}` where gates is `[{id, baseUrl, bearerToken}, ...]`.
    pub peers: String,

    /// HTTP method dispatch key, same vocabulary as the `http.*` step
    /// (`http.get`, `http.post`, `http.put`, `http.patch`,
    /// `http.delete`). Default `http.get`.
    #[serde(default = "default_parallel_http_call")]
    pub call: String,

    /// Per-peer HTTP args. `url`, `body`, `headers`, `query` can
    /// reference `${peer.*}` to template per-peer values. Resolved
    /// once per peer in the parent context before the fan-out spawns.
    pub args: HttpArgs,

    /// Per-peer deadline in milliseconds. Covers the full outbound
    /// request round-trip (connect + send + first byte). Each peer
    /// honours this independently; a slow peer times out without
    /// affecting the others. `None` falls back to `AppConfig.http_request_timeout`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// Bounded fan-out concurrency. `None` means "no explicit cap"
    /// (bounded by `peers.len()` in practice). Operators serving
    /// public traffic should always set a modest value to prevent one
    /// request from spawning `len(peers)` outbound connections at
    /// once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,

    /// Aggregation mode. `collect_ok` drops errored peers; `collect_all`
    /// keeps everything; `first_n` returns early when `first_n` peers
    /// have satisfied `early_exit_on`.
    pub aggregate: AggregateMode,

    /// Required when `aggregate: first_n`. Target number of successes
    /// before the step unblocks. Parse-time error if present under
    /// `collect_ok` / `collect_all`, or absent under `first_n`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_n: Option<u32>,

    /// Only valid under `aggregate: first_n`. Defines which peer
    /// responses count toward the `first_n` quota. Absent → the
    /// default is 2xx responses with no body predicate (every 2xx
    /// counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_exit_on: Option<EarlyExitOn>,

    /// Only valid under `aggregate: first_n`. Disposition for peers
    /// still in flight once `first_n` is satisfied. Default `cancel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_peers_after: Option<RemainingPeersAfter>,

    /// Name of the variable the aggregated array is bound to in the
    /// parent context.
    pub result: String,
}

/// Issue #135 — aggregation mode for `parallel_http`. See
/// `book/src/dsl/steps/parallel_http.md` for the full semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    /// Wait for every peer; keep successes (2xx + transport-OK) only.
    CollectOk,
    /// Wait for every peer; keep errors too — array shape stays stable
    /// and errored peers surface with `response.error` populated.
    CollectAll,
    /// Return as soon as `first_n` peers satisfy `early_exit_on`.
    FirstN,
}

/// Issue #136 — predicate that decides which responses count toward
/// `first_n`. Both fields optional; absent fields default to permissive.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EarlyExitOn {
    /// Inclusive HTTP status range `[low, high]` a response must fall
    /// into to count toward `first_n`. Default `[200, 299]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_range: Option<[u16; 2]>,

    /// Optional JS expression evaluated per peer response. The response
    /// is bound to `${response}` (shape: `{status, body, headers,
    /// error}`). Must return truthy for the response to count toward
    /// `first_n`. Example: `"${response.body.found === true}"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_predicate: Option<String>,
}

/// Issue #136 — what to do with peers still in-flight after `first_n`
/// is satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemainingPeersAfter {
    /// Abort the remaining tasks (hyper closes the sockets). Lowest
    /// tail latency. Caller sees only the `first_n` matches.
    Cancel,
    /// Detach the remaining tasks — they keep running in a
    /// `tokio::spawn` and their eventual results are logged (not
    /// collected). Caller continues immediately with the first_n
    /// matches. Use for audit-while-serving.
    DrainBg,
}

fn default_parallel_http_call() -> String {
    "http.get".to_string()
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
    /// Response headers — either a YAML mapping with per-key values
    /// (each value may contain `${…}` expressions), or a single
    /// `${expr}` string that evaluates to an object at runtime.
    /// Issue #25.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Value>,
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
    /// Query params — either a YAML mapping with per-key values
    /// (each value may contain `${…}` expressions), or a single
    /// `${expr}` string that evaluates to an object at runtime.
    /// Issue #25 — accepting only a mapping here forced DSL authors
    /// to inline every key literally, defeating computed / merged
    /// param maps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<Value>,
    /// Headers — either a YAML mapping with per-key values (each
    /// value may contain `${…}` expressions), or a single `${expr}`
    /// string that evaluates to an object at runtime. Same
    /// rationale as `query`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Value>,
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
    /// Issue #56 — accepts any JSON-shaped value: a scalar string
    /// (the classic single-line form), a mapping (structured key/value
    /// log payload), or an array. Every string leaf runs through the
    /// script engine so `${…}` interpolation works the same as
    /// `assign:`, `template.body:`, `http.args.body:`. Rendering to
    /// the log sink stays "string → as-is, else compact JSON",
    /// sanitised for CR/LF and truncated at 256 chars.
    pub log: Value,
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
    /// Optional tag-based broadcast filter. If set, it takes priority
    /// over both `to` and `broadcast_prefix`: the payload goes to
    /// every connection whose tag (set earlier via `ws_tag`) matches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcast_where: Option<BroadcastWhere>,
}

/// Tag predicate for `ws_send: { broadcast_where: … }`.
///
/// ```yaml
/// ws_send:
///   broadcast_where:
///     tag: "roles"
///     contains: "admin"        # or: equals: "admin"
///   payload: { type: "ping" }
/// ```
///
/// `tag`, and the operand of `equals` / `contains`, are all evaluated
/// through the script engine first, so `${…}` expressions work
/// (`contains: "${incoming.body.required_role}"`). A connection with
/// no such tag never matches.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BroadcastWhere {
    /// Tag key to test.
    pub tag: Value,
    /// Exact-match operand. The tag value must equal this string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    /// Substring operand. The tag value must contain this string.
    /// Store list-valued tags with surrounding delimiters
    /// (`",admin,ops,"`) and match `",admin,"` for token-exact
    /// semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsTagStep {
    pub ws_tag: WsTagArgs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(flatten)]
    pub base: BaseStepFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsTagArgs {
    /// Tag key → value-expression map. Each value is evaluated through
    /// the script engine against the current context, coerced to a
    /// string, and stored on the connection this DSL run was
    /// triggered by (`context.connection_id()`). Only valid inside a
    /// WS server DSL. Existing tags with the same key are overwritten;
    /// others are left untouched.
    pub set: std::collections::BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateStep {
    pub template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,
    /// Body — either a YAML mapping with per-key values (each may
    /// contain `${…}` expressions) or a single `${expr}` string that
    /// evaluates to an object at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Query — same shape as `body`. Mapping or `${expr}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<Value>,
    /// Headers — same shape as `body`. Evaluated values are coerced
    /// to strings for the callee.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Value>,
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
    /// Issue #37 — per-step-type diagnostics the engine folds into
    /// the "Executed" INFO line. Each executor pushes the small set
    /// of fields that make its outcome self-describing (HTTP: URL +
    /// upstream status; switch: matched branch; return: final status;
    /// state: op + key; log: message; iterate: item count). Empty for
    /// steps whose type alone tells the whole story. Rendered by
    /// [`StepLogExtras::Display`] into the `attrs` field of the log
    /// line, sanitised for log-line safety.
    pub log_extras: StepLogExtras,
}

/// One entry in [`StepLogExtras`]. The distinction lets Display
/// know whether to add `"…"` quoting: [`StepLogEntry::Value`]
/// wraps string values in quotes so `foo bar` round-trips as a
/// single value, while [`StepLogEntry::Preformatted`] emits the
/// text verbatim (used for values that are already self-quoting,
/// e.g. a JSON preview like `{"a":1}`).
#[derive(Debug, Clone)]
pub enum StepLogEntry {
    Value(Value),
    Preformatted(String),
}

/// Ordered `(name, entry)` pairs a step executor exposes to the
/// engine's per-step INFO log line (issue #37). Preserves push order
/// so the rendered `attrs=` field is stable across runs. Keys are
/// `&'static str` (semantic-convention field names) to keep the
/// enrichment site zero-allocation for the common case.
#[derive(Debug, Clone, Default)]
pub struct StepLogExtras(pub Vec<(&'static str, StepLogEntry)>);

impl StepLogExtras {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Push a `(key, value)` pair. Fluent to keep executor call sites
    /// terse: `StepLogExtras::new().push("k1", v1).push("k2", v2)`.
    /// String values get wrapped in `"..."` by Display so values with
    /// embedded spaces round-trip. Use [`Self::push_preformatted`]
    /// when the value is already self-quoting (e.g. a JSON preview) —
    /// otherwise the reader sees `body=""pong""` (JSON's own quotes
    /// plus the StepLogExtras wrapping).
    pub fn push(mut self, key: &'static str, value: impl Into<Value>) -> Self {
        self.0.push((key, StepLogEntry::Value(value.into())));
        self
    }

    /// Push a `(key, value)` pair whose value is already a
    /// self-describing textual form (e.g. JSON preview from
    /// `crate::logging::preview_body_for_log`). Bypasses the string
    /// quoting Display would otherwise apply, so the reader sees
    /// `return.body={"items":[…]}` instead of the double-quoted
    /// `return.body="{\"items\":[…]}"`. Sanitisation (CR/LF strip)
    /// still applies at Display time.
    pub fn push_preformatted(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.0.push((key, StepLogEntry::Preformatted(value.into())));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for StepLogExtras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `k1=v1 k2=v2` — space-separated so a text-format log line
        // stays readable, string values quoted (and CR/LF stripped)
        // so an attacker-controlled URL or message can't splice a
        // fake log line into the stream. JSON-format consumers see
        // the same compact rendering under `attrs`.
        let mut first = true;
        for (k, entry) in &self.0 {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            match entry {
                StepLogEntry::Value(Value::String(s)) => {
                    let cleaned = crate::logging::sanitize_log_value(s);
                    write!(f, "{}=\"{}\"", k, cleaned)?;
                }
                StepLogEntry::Value(Value::Null) => write!(f, "{}=null", k)?,
                StepLogEntry::Value(v) => write!(f, "{}={}", k, v)?,
                StepLogEntry::Preformatted(s) => {
                    let cleaned = crate::logging::sanitize_log_value(s);
                    write!(f, "{}={}", k, cleaned)?;
                }
            }
        }
        Ok(())
    }
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

#[cfg(test)]
mod log_extras_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_empty_renders_nothing() {
        let extras = StepLogExtras::new();
        assert_eq!(extras.to_string(), "");
        assert!(extras.is_empty());
    }

    #[test]
    fn display_string_values_are_quoted() {
        let extras = StepLogExtras::new()
            .push("url.full", "https://example.com/x")
            .push("http.response.status_code", 200u16);
        // Order-preserving, strings quoted, non-strings bare.
        assert_eq!(
            extras.to_string(),
            r#"url.full="https://example.com/x" http.response.status_code=200"#
        );
    }

    #[test]
    fn display_strips_crlf_from_string_values() {
        // Attacker-controlled newline in a URL / log message must
        // NOT splice a fake log line into the stream.
        let extras = StepLogExtras::new().push("url.full", "legit\ninjected");
        let rendered = extras.to_string();
        assert!(!rendered.contains('\n'), "no raw CR/LF: {}", rendered);
        assert!(
            rendered.contains(' '),
            "CRLF replaced by space: {}",
            rendered
        );
    }

    #[test]
    fn display_preformatted_values_are_not_double_quoted() {
        // Regression: prior version wrapped every string value in
        // "…", including preview values that were already JSON-
        // encoded, giving `body=""pong""` (JSON's quotes + our
        // wrapping). push_preformatted skips the outer wrap.
        let extras = StepLogExtras::new()
            .push_preformatted("return.body", "\"pong\"")
            .push_preformatted("state.value", "{\"a\":1}");
        assert_eq!(
            extras.to_string(),
            "return.body=\"pong\" state.value={\"a\":1}"
        );
    }

    #[test]
    fn display_preformatted_strips_crlf() {
        // Even preformatted values must not splice log lines via
        // an embedded newline.
        let extras = StepLogExtras::new().push_preformatted("return.body", "legit\ninjected");
        assert!(!extras.to_string().contains('\n'));
    }

    #[test]
    fn display_handles_null_and_object_values() {
        let extras = StepLogExtras::new()
            .push("state.hit", Value::Null)
            .push("state.key", json!("counter"));
        let rendered = extras.to_string();
        assert!(
            rendered.contains("state.hit=null"),
            "null literal: {}",
            rendered
        );
        assert!(rendered.contains("state.key=\"counter\""));
    }
}

#[cfg(test)]
mod step_keys_tests {
    use super::{ACTION_STEP_KEYS, STEP_KEYS};
    use std::collections::HashSet;

    /// Issue #82 — pin the invariant that `ACTION_STEP_KEYS` is
    /// exactly `STEP_KEYS` minus `"declaration"`. Adding a new step
    /// primitive without keeping the two lists aligned trips this
    /// test, catching the class of drift that PR #80 fixed (`ws_tag:`
    /// landed in the parser only for two release cycles).
    #[test]
    fn action_step_keys_is_step_keys_minus_declaration() {
        let all: HashSet<&str> = STEP_KEYS.iter().copied().collect();
        let actions: HashSet<&str> = ACTION_STEP_KEYS.iter().copied().collect();
        let expected: HashSet<&str> = all
            .iter()
            .copied()
            .filter(|k| *k != "declaration")
            .collect();
        assert_eq!(
            actions, expected,
            "ACTION_STEP_KEYS drifted from STEP_KEYS \\ {{declaration}} — \
             adding a new step primitive requires updating both lists in \
             `src/steps/mod.rs`."
        );
    }

    /// Sanity: every step key must be a valid YAML identifier (no
    /// spaces, no punctuation) — otherwise `contains_key` in the
    /// parser and linter wouldn't match anything.
    #[test]
    fn step_keys_are_valid_yaml_identifiers() {
        for k in STEP_KEYS {
            assert!(
                k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'),
                "step key {k:?} contains a character that won't parse as a YAML key"
            );
        }
    }
}

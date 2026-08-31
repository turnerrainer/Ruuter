use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_config_path")]
    pub config_path: PathBuf,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub stop_in_case_of_exception: bool,

    #[serde(default)]
    pub http_codes_allow_list: Vec<u16>,

    /// Cap on the number of step transitions a single DSL run may perform
    /// before the engine aborts the run. Guards against infinite `next:`
    /// loops. `iterate` has its own per-step bound; this is a sanity cap on
    /// top-level transitions.
    #[serde(default)]
    pub max_step_recursions: Option<u32>,

    #[serde(default)]
    pub http_response_size_limit: Option<usize>,

    #[serde(default = "default_http_request_timeout")]
    pub http_request_timeout: u64,

    #[serde(default)]
    pub cors: CorsConfig,

    #[serde(default)]
    pub dsl: DslConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub incoming_requests: IncomingRequestsConfig,

    #[serde(default)]
    pub response_default_headers: HashMap<String, String>,

    /// Audit finding 12 — response wrapper. When true (the default)
    /// and the DSL's terminating `return:` step didn't set `wrapper:
    /// false`, the response body is wrapped in
    /// `{"response": <value>}` — Java-parity `RuuterResponse` shape.
    /// Set to `false` for raw unwrapped bodies. A ReturnStep's
    /// explicit `wrapper: true|false` always wins over this config.
    #[serde(default)]
    pub response: ResponseConfig,

    /// Audit finding 13 — Java `defaultDslInCaseOfException` parity.
    /// When set, an HTTP step whose status falls outside
    /// `http_codes_allow_list` AND has no local `error:` step will
    /// invoke this fallback DSL. The fallback receives an enriched
    /// body containing `statusCode`, `responseBody`, and
    /// `failedRequestId` (traceparent trace id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dsl_in_case_of_exception: Option<DefaultHttpDslConfig>,

    /// Audit finding 14 — guard evaluation mode. Default `Stack`
    /// (current Rust behaviour: every ancestor guard runs
    /// outer-first). `ClosestOnly` matches Java's `DslService.getGuard`
    /// which runs ONLY the innermost ancestor guard. Guards with
    /// `override_ancestors: true` still override in either mode.
    #[serde(default)]
    pub guards: GuardsConfig,

    #[serde(default)]
    pub internal_requests: InternalRequestsConfig,

    #[serde(default)]
    pub csrf: CsrfConfig,

    #[serde(default)]
    pub proxy: ProxyConfig,

    #[serde(default)]
    pub scripting: ScriptingConfig,

    #[serde(default)]
    pub optimistic_concurrency: OptimisticConcurrencyConfig,

    /// Task 043 — outbound Unix-domain-socket transport aliases.
    ///
    /// Maps `http://<host>/...` URLs whose host matches a key to a
    /// UDS transport hitting the mapped socket path. Lets DSL
    /// authors keep writing `http://resql/query` (no protocol
    /// change) while the operator swings the transport from TCP
    /// loopback to a Unix socket via config only. The `unix://`
    /// URL scheme is also supported directly, but the alias form
    /// is preferred because it keeps DSLs portable across
    /// TCP-only and UDS-enabled deployments.
    ///
    /// Example:
    /// ```yaml
    /// unix_socket_map:
    ///   resql:  "/var/run/ruuter/resql.sock"
    ///   tim:    "/var/run/ruuter/tim.sock"
    /// ```
    #[serde(default)]
    pub unix_socket_map: HashMap<String, PathBuf>,

    /// Task 049 — HTTP version for outbound UDS calls.
    /// `Http1` (default) uses HTTP/1.1 with keep-alive (task 050).
    /// `Http2` uses h2c (HTTP/2 cleartext) with stream multiplexing
    /// on a single connection — significantly higher throughput
    /// under concurrent-request load. Sidecars must speak h2c to
    /// benefit.
    #[serde(default)]
    pub uds_http_version: HttpVersion,

    /// Task 043 — inbound listeners. When present, this list REPLACES
    /// the default single TCP listener on `port`. Each listener spawns
    /// its own accept loop; the same axum Router serves all of them.
    ///
    /// Use case: expose external traffic on TCP and internal /admin
    /// or fast-path traffic on a Unix socket, without a separate
    /// process. When absent, the framework falls back to a single
    /// TCP listener bound to `0.0.0.0:<port>` (0.4.0 behaviour).
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
}

/// One inbound-listener binding. Exactly one of `bind` or `unix` must
/// be set; both-or-neither is a config error caught at boot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListenerConfig {
    /// Optional label surfaced in startup logs — helps operators
    /// tell listeners apart at a glance. Defaults to the transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// TCP bind spec (`host:port`). Mutually exclusive with `unix`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,

    /// Unix-socket path. Mutually exclusive with `bind`. The path is
    /// removed if it exists before binding (stale sockets from a
    /// crashed prior instance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix: Option<PathBuf>,

    /// Task 049 — accept HTTP/2 cleartext (h2c) on this listener.
    /// Default false = HTTP/1.1 only. When true, connections use
    /// hyper's http2 server builder instead of http1. Recommended for
    /// UDS listeners paired with h2c outbound clients.
    #[serde(default)]
    pub http2: bool,
}

/// Task 049 — HTTP version selector for outbound calls. Serialised
/// as lowercase strings so YAML config is natural (`http1`, `http2`).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HttpVersion {
    #[default]
    Http1,
    Http2,
}

/// Audit finding 14 — guard evaluation mode.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GuardsConfig {
    #[serde(default)]
    pub mode: GuardMode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuardMode {
    /// Current Rust behaviour: every matching ancestor guard runs
    /// outer-first. Safer default (more checks). Preserved for
    /// operators who rely on stacked guards.
    #[default]
    Stack,
    /// Java parity: only the closest (longest-key) ancestor guard
    /// runs. Ancestor guards are silently skipped.
    ClosestOnly,
}

/// Audit finding 13 — Java `DefaultHttpDsl` shape. Names an HTTP DSL
/// (routed as `<request_type> /<project>/<dsl>`) to invoke when an
/// upstream call errors out and no local `error:` step is set.
///
/// `body`, `query`, and `headers` are supplied verbatim to the
/// fallback DSL. The framework enriches `body` with `statusCode`,
/// `responseBody`, and `failedRequestId` (traceparent trace id)
/// keys before dispatch (Java's `DefaultHttpDsl.executeHttpDefaultDsl`).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DefaultHttpDslConfig {
    /// DSL name relative to the project. Java: `dsl:` field.
    pub dsl: String,
    /// HTTP method used to look up the fallback DSL. Defaults to
    /// POST (matches Java's `requestType` default in samples).
    #[serde(default = "default_exception_request_type", alias = "requestType")]
    pub request_type: String,
    /// Project the fallback DSL lives under. Rust has a project
    /// layer that Java doesn't — operators explicitly name it here.
    /// Defaults to "framework" so operators can drop a
    /// `DSL/framework/POST/default-dsl.yml` and reference it by
    /// bare `dsl: default-dsl`.
    #[serde(default = "default_exception_project")]
    pub project: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub body: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub query: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

fn default_exception_request_type() -> String {
    "POST".to_string()
}
fn default_exception_project() -> String {
    "framework".to_string()
}

/// Audit finding 12 — response-shape defaults. See `AppConfig::response`
/// for the wrapper default semantics; other fields (dsl-with-response
/// / -without-response status codes) mirror Java's `finalResponse`
/// block from `application.yml` (finding 13).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponseConfig {
    /// Default wrapper mode when the ReturnStep doesn't specify.
    /// `true` (default) wraps in `{"response": <value>}` — matches
    /// Java Ruuter's `RuuterResponse` shape. `false` returns raw
    /// body. A step-level `wrapper: X` always wins over this.
    #[serde(default = "default_response_wrapper")]
    pub default_wrapper: bool,

    /// Audit finding 13 — Java `finalResponse.dslWithResponseHttpStatusCode`:
    /// status returned when the DSL's `return:` step emitted a value
    /// and didn't set an explicit `status:`. `None` = 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsl_with_response_status: Option<u16>,

    /// Audit finding 13 — Java `finalResponse.dslWithoutResponseHttpStatusCode`:
    /// status returned when the DSL never reached a `return:` step
    /// (loop-cap exhaustion, empty pipeline, all steps skipped).
    /// `None` = 200. Java's sample sets 300; operators picking that
    /// pattern get an explicit "no body" signal separate from a
    /// happy-path 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsl_without_response_status: Option<u16>,
}

fn default_response_wrapper() -> bool {
    true
}

impl Default for ResponseConfig {
    fn default() -> Self {
        Self {
            default_wrapper: default_response_wrapper(),
            dsl_with_response_status: None,
            dsl_without_response_status: None,
        }
    }
}

/// Framework hook for PATTERNS.md §3 (If-Match / ETag). The actual ETag
/// value is aggregate-specific (Resql state_id) so Ruuter cannot validate
/// it — but it CAN reject state-changing requests that arrive without any
/// If-Match header at all. The DSL then validates the token against its
/// aggregate state via a Resql query.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct OptimisticConcurrencyConfig {
    /// When true, PUT / PATCH / DELETE without an `If-Match` header get
    /// 428 Precondition Required (RFC 6585). Default false — the DSL is
    /// still responsible for actually validating the token.
    #[serde(default)]
    pub require_if_match: bool,

    /// Methods that must carry `If-Match`. Only consulted when
    /// `require_if_match` is true.
    #[serde(default = "default_ifmatch_methods")]
    pub enforce_on_methods: Vec<String>,
}

fn default_ifmatch_methods() -> Vec<String> {
    vec!["PUT".to_string(), "PATCH".to_string(), "DELETE".to_string()]
}

/// Server-side CSRF stance for state-changing methods. Implements
/// PATTERNS.md §1: an Origin/Referer allow-list check on POST/PUT/PATCH/DELETE.
/// When `allowed_origins` is empty the check is bypassed — useful for
/// single-tenant admin surfaces that are behind a same-origin reverse
/// proxy and rely on `SameSite=Strict` cookies alone.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CsrfConfig {
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Methods that trigger the Origin/Referer check. Default:
    /// POST/PUT/PATCH/DELETE.
    #[serde(default = "default_csrf_methods")]
    pub enforce_on_methods: Vec<String>,
}

fn default_csrf_methods() -> Vec<String> {
    vec![
        "POST".to_string(),
        "PUT".to_string(),
        "PATCH".to_string(),
        "DELETE".to_string(),
    ]
}

/// Guardrails for the embedded JavaScript engine. The Boa runtime is
/// synchronous and cannot be cooperatively cancelled once evaluation
/// begins, so we impose CPU-bounded caps rather than wall-clock ones.
///
/// A DSL author writing `${while(true){}}` will hit `max_loop_iterations`
/// and fail the evaluation rather than hang the tokio worker.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptingConfig {
    /// Boa loop-iteration cap per `${...}` / `$= ... =$` evaluation.
    #[serde(default = "default_script_max_loop_iterations")]
    pub max_loop_iterations: u64,

    /// Maximum JS call stack depth. Guards against unbounded recursion.
    #[serde(default = "default_script_max_stack_size")]
    pub max_stack_size: usize,
}

fn default_script_max_loop_iterations() -> u64 {
    1_000_000
}
fn default_script_max_stack_size() -> usize {
    400
}

impl Default for ScriptingConfig {
    fn default() -> Self {
        Self {
            max_loop_iterations: default_script_max_loop_iterations(),
            max_stack_size: default_script_max_stack_size(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub allow_credentials: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DslConfig {
    #[serde(default = "default_allowed_filetypes")]
    pub allowed_filetypes: Vec<String>,

    #[serde(default = "default_processed_filetypes")]
    pub processed_filetypes: Vec<String>,

    /// Dev-only opt-in for the DSL hot-reload filesystem watcher.
    /// When `true`, a `notify`-backed watcher on `config_path`
    /// republishes the HTTP + guard trees on change without a server
    /// restart. Default `false`. Do NOT enable in production —
    /// combined with a writable DSL mount it is RCE via `${JS}`
    /// expressions. See `book/src/ops/hot-reload.md` for the full
    /// list of what does / does not reload.
    #[serde(default)]
    pub allow_dsl_reloading: bool,

    /// Task 070 — emit one WARN per HTTP-routed DSL that has no
    /// `declaration:` block. Missing declaration is NEVER an error
    /// (the DSL still loads and runs — matches Java-parity permissive
    /// posture); the WARN is the operator's signal that OpenAPI
    /// generation, per-field allowlisting, and typed request/response
    /// schemas are unavailable for that route. Default `true` so
    /// operators moving from Resql (which mandates declarations)
    /// see the gap. Flip to `false` if the corpus intentionally runs
    /// without declarations.
    #[serde(default = "default_true")]
    pub warn_on_missing_declaration: bool,
}

/// Log output format. `Text` (default) is `tracing`'s human-friendly
/// single-line renderer — great for local dev and container logs
/// scraped by grep. `Json` emits one JSON object per event and is the
/// recommended production shape: Loki, Elastic, CloudWatch, Datadog
/// all key on structured fields without per-service regex parsing.
///
/// Env override: `RUUTER_LOG_FORMAT=text|json` wins over the config
/// file so operators can flip a running container without editing
/// `ruuter.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Default. Compact one-line-per-event text output tuned for
    /// terminal readability: `HH:MM:SS.mmm LEVEL [t=<8hex> <project>]
    /// <message> <fields>`. Span noise (Rust module target, OTel span
    /// fields duplicated on children) is filtered out; OTLP span
    /// export (when enabled) still sees the full field set.
    #[default]
    Text,
    /// One JSON object per event. Recommended for production ingest
    /// (Loki / Elastic / CloudWatch / Datadog); field-indexed at ingest
    /// without regex parsing. Full span field set preserved.
    Json,
    /// Terminal-first output for interactive local dev: ANSI-coloured
    /// (level, step type, duration) with visual step markers.
    /// Otherwise identical schema to `text`. Colours are unconditional
    /// — enable only when the terminal will render them (avoid
    /// piping to files or log aggregators).
    Pretty,
}

fn default_max_body_bytes() -> usize {
    2048
}

/// Header names redacted by default from every logged HTTP request /
/// response header map. Names are compared case-insensitively.
fn default_redact_headers() -> Vec<String> {
    vec![
        "authorization".into(),
        "proxy-authorization".into(),
        "cookie".into(),
        "set-cookie".into(),
        "x-api-key".into(),
        "x-auth-token".into(),
    ]
}

/// Body-field names redacted by default from every logged JSON body.
/// Matched case-insensitively at any nesting depth.
fn default_redact_body_fields() -> Vec<String> {
    vec![
        "password".into(),
        "pass".into(),
        "secret".into(),
        "token".into(),
        "access_token".into(),
        "refresh_token".into(),
        "api_key".into(),
        "authorization".into(),
    ]
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// Include outbound HTTP request bodies in per-step DEBUG log
    /// lines. Redacted (see `redact_body_fields`) and capped to
    /// `max_body_bytes`. Off by default — bodies routinely carry
    /// PII / secrets.
    #[serde(default)]
    pub display_request_content: bool,

    /// Include upstream HTTP response bodies in per-step DEBUG log
    /// lines. Redacted and capped the same way as
    /// `display_request_content`.
    #[serde(default)]
    pub display_response_content: bool,

    /// On step error, include the error's `source()` chain on the
    /// ERROR log line. Off by default — the top-level `Display` is
    /// usually enough and chains can leak upstream schema details.
    #[serde(default)]
    pub print_stack_trace: bool,

    /// On step error, emit a second WARN line with the underlying
    /// cause message. Off by default — the primary ERROR line is
    /// usually enough. Named after Java Ruuter's flag of the same
    /// meaning.
    #[serde(default)]
    pub meaningful_errors: bool,

    /// Log output format. Default `text`; `json` emits one JSON
    /// object per event. Env `RUUTER_LOG_FORMAT` overrides.
    #[serde(default)]
    pub format: LogFormat,

    /// Emit one INFO line per completed HTTP request with method,
    /// route, status, duration, trace_id, project, origin. On by
    /// default — this is the operational access log every service
    /// should have.
    #[serde(default = "default_true")]
    pub access_log: bool,

    /// Emit one DEBUG line per DSL step with step name / type /
    /// elapsed. Off by default — chatty for high-QPS DSLs.
    #[serde(default)]
    pub step_timing: bool,

    /// Emit one INFO `Executed` line per DSL step with step name,
    /// step type, elapsed time, next-step target, and per-step-type
    /// context (HTTP: URL + upstream status; switch: matched branch;
    /// return: final status; state: op + key; log: message; iterate:
    /// item count). ON by default — this is the DSL-execution trail
    /// Java Ruuter's `LoggingUtils.logStep()` emitted at INFO. Turn
    /// off for very high-QPS DSLs where the per-step line becomes
    /// noise; DEBUG-level `step_timing` remains a finer-grained
    /// alternative. Corresponds to issue #37 (per-step visibility).
    #[serde(default = "default_true")]
    pub log_step_executions: bool,

    /// Emit one INFO line at the start and end of every DSL run
    /// (dsl.project + total_steps / duration_ms / terminated_by).
    /// OFF by default — the request span already brackets each run
    /// via `trace_id`, and the access log carries the same wall-
    /// clock duration + response status, so the bracket lines are
    /// largely redundant for a single-step DSL. Turn on when you
    /// want an explicit `terminated_by` label (`return` /
    /// `end_of_steps` / `iteration_cap` / `error`) in the log stream
    /// without walking the per-step trail — useful for grep-based
    /// triage of long DSLs.
    #[serde(default)]
    pub log_dsl_runs: bool,

    /// Cap on any body content included in a log line. Defaults to
    /// 2 KiB — enough to identify the shape without shipping full
    /// payloads to the log store.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,

    /// Header names replaced with `"[REDACTED]"` in any logged
    /// header map. Case-insensitive. Defaults cover common auth /
    /// session headers; add project-specific names as needed.
    #[serde(default = "default_redact_headers")]
    pub redact_headers: Vec<String>,

    /// JSON body field names replaced with `"[REDACTED]"` in any
    /// logged body. Case-insensitive, applied at every nesting
    /// depth. Defaults cover common secret-bearing field names.
    #[serde(default = "default_redact_body_fields")]
    pub redact_body_fields: Vec<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            display_request_content: false,
            display_response_content: false,
            print_stack_trace: false,
            meaningful_errors: false,
            format: LogFormat::default(),
            access_log: true,
            step_timing: false,
            log_step_executions: true,
            log_dsl_runs: false,
            max_body_bytes: default_max_body_bytes(),
            redact_headers: default_redact_headers(),
            redact_body_fields: default_redact_body_fields(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncomingRequestsConfig {
    #[serde(default = "default_allowed_methods")]
    pub allowed_method_types: Vec<String>,

    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// h2ck.me S4 — trusted reverse-proxy list. Only requests whose
/// direct TCP peer IP is in `trusted` may have `X-Forwarded-For` /
/// `X-Real-IP` promoted into the DSL's `incoming.origin` field. From
/// untrusted peers those headers are still visible via
/// `incoming.headers` (some DSLs log or hash them), but the
/// framework-level `origin` — which downstream code uses for audit,
/// rate-limit keys, and self-call bookkeeping — reflects the socket
/// peer instead. Empty list (default) means no proxy is trusted,
/// which is the safest posture for direct-exposed deployments.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProxyConfig {
    #[serde(default)]
    pub trusted: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InternalRequestsConfig {
    #[serde(default)]
    pub disabled: bool,

    #[serde(default)]
    pub allowed_ips: Vec<String>,

    #[serde(default)]
    pub allowed_urls: Vec<String>,

    /// h2ck.me N4 — defense-in-depth against cloud-metadata SSRF.
    /// When true (default), outbound TCP requests targeting
    /// link-local (169.254/16, fe80::/10), loopback (127/8, ::1),
    /// unspecified (0.0.0.0, ::), or RFC-1918 / ULA private ranges
    /// (10/8, 172.16/12, 192.168/16, fc00::/7) are rejected before
    /// dispatch. Self-call short-circuits (URLs pointing at the
    /// framework's own listener) and UDS transports (`unix://` scheme,
    /// `unix_socket_map` alias) are unaffected — those never touch
    /// TCP.
    ///
    /// Set to false to restore the pre-v0.7 permissive behaviour if
    /// you legitimately need a DSL to call a private-network sidecar
    /// over TCP loopback without migrating to `unix_socket_map`.
    #[serde(default = "default_block_private_networks")]
    pub block_private_networks: bool,
}

fn default_block_private_networks() -> bool {
    true
}

impl Default for InternalRequestsConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            allowed_ips: Vec::new(),
            allowed_urls: Vec::new(),
            block_private_networks: default_block_private_networks(),
        }
    }
}

fn default_config_path() -> PathBuf {
    PathBuf::from("./DSL")
}

fn default_port() -> u16 {
    8080
}

fn default_http_request_timeout() -> u64 {
    15000
}

fn default_allowed_filetypes() -> Vec<String> {
    vec![".yml".to_string(), ".yaml".to_string()]
}

fn default_processed_filetypes() -> Vec<String> {
    vec![".yml".to_string(), ".yaml".to_string()]
}

fn default_allowed_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "PATCH".to_string(),
        "DELETE".to_string(),
        "OPTIONS".to_string(),
    ]
}

impl Default for DslConfig {
    fn default() -> Self {
        Self {
            allowed_filetypes: default_allowed_filetypes(),
            processed_filetypes: default_processed_filetypes(),
            allow_dsl_reloading: false,
            warn_on_missing_declaration: true,
        }
    }
}

impl Default for IncomingRequestsConfig {
    fn default() -> Self {
        Self {
            allowed_method_types: default_allowed_methods(),
            headers: HashMap::new(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_path: default_config_path(),
            port: default_port(),
            stop_in_case_of_exception: true,
            // Empty = accept every upstream status (matches Java default).
            http_codes_allow_list: Vec::new(),
            max_step_recursions: Some(10_000),
            http_response_size_limit: Some(16 * 1024 * 1024),
            http_request_timeout: default_http_request_timeout(),
            cors: CorsConfig::default(),
            dsl: DslConfig::default(),
            logging: LoggingConfig::default(),
            incoming_requests: IncomingRequestsConfig::default(),
            response_default_headers: HashMap::new(),
            internal_requests: InternalRequestsConfig::default(),
            csrf: CsrfConfig::default(),
            proxy: ProxyConfig::default(),
            scripting: ScriptingConfig::default(),
            optimistic_concurrency: OptimisticConcurrencyConfig::default(),
            unix_socket_map: HashMap::new(),
            uds_http_version: HttpVersion::Http1,
            listeners: Vec::new(),
            response: ResponseConfig::default(),
            default_dsl_in_case_of_exception: None,
            guards: GuardsConfig::default(),
        }
    }
}

/// Where to look for the operator's config file, in priority order.
/// The first path that exists wins; if none exist, `AppConfig::default()`
/// is returned by the caller.
fn config_search_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // 1. --config <path>
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--config" && i + 1 < args.len() {
            out.push(PathBuf::from(&args[i + 1]));
            break;
        }
        if let Some(rest) = args[i].strip_prefix("--config=") {
            out.push(PathBuf::from(rest));
            break;
        }
        i += 1;
    }
    // 2. RUUTER_CONFIG env
    if let Ok(p) = std::env::var("RUUTER_CONFIG") {
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    // 3. conventional cwd locations
    out.push(PathBuf::from("./ruuter.yaml"));
    out.push(PathBuf::from("./ruuter.yml"));
    out
}

impl AppConfig {
    /// Resolve, load, and return the operator's AppConfig. Falls back to
    /// `AppConfig::default()` when no config file is found on any of the
    /// conventional paths.
    ///
    /// Returns a tuple `(config, source)` — `source` is `Some(path)` when
    /// a file was loaded, `None` when defaults were used. Caller logs the
    /// choice at INFO so ops can see which config took effect.
    pub fn load_or_default() -> crate::Result<(Self, Option<PathBuf>)> {
        for path in config_search_paths() {
            if path.exists() {
                let body = std::fs::read_to_string(&path).map_err(|e| {
                    crate::RuuterError::Config(format!(
                        "reading config file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                let cfg: AppConfig = serde_yaml_ng::from_str(&body).map_err(|e| {
                    crate::RuuterError::Config(format!(
                        "parsing config file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                return Ok((cfg, Some(path)));
            }
        }
        Ok((Self::default(), None))
    }
}

/// Audit finding 15 — startup warning for config fields that the
/// framework accepts (so operators can port a Java `application.yml`
/// as-is) but doesn't yet wire end-to-end. Each warn line names the
/// affected field AND the intended contract so an operator can tell
/// "not implemented" from "wrong value."
///
/// Runs once at boot from main.rs, AFTER config load and BEFORE
/// listener startup, so the signal is visible in the same log
/// stream as the "Loaded config from …" line.
pub fn warn_on_stale_config_fields(config: &AppConfig) {
    // stop_in_case_of_exception=false is un-honoured (the engine
    // propagates every step error via `?`, so it always stops).
    if !config.stop_in_case_of_exception {
        tracing::warn!(
            "config: stop_in_case_of_exception=false is not honoured — the engine \
             always halts a run on step error (Java's continue-on-error semantics \
             are not implemented). Remove the setting or leave the default (true)."
        );
    }

    // logging.* flags — as of the observability chapter (see
    // book/src/logging/) all four are wired end-to-end.
    // No WARN emitted regardless of value.

    // allowed_filetypes vs processed_filetypes — pre-fix Rust only
    // reads processed_filetypes. Warn when they differ so operators
    // know allowed_filetypes was silently the same list.
    let allowed: std::collections::HashSet<_> = config.dsl.allowed_filetypes.iter().collect();
    let processed: std::collections::HashSet<_> = config.dsl.processed_filetypes.iter().collect();
    if allowed != processed {
        tracing::warn!(
            "config: dsl.allowed_filetypes differs from dsl.processed_filetypes — \
             the loader only consults processed_filetypes. allowed_filetypes is a \
             Java-parity noun that has no gating effect. Fold the two into \
             processed_filetypes or accept that allowed_filetypes is inert."
        );
    }
}

pub fn load_constants(path: &str) -> crate::Result<HashMap<String, String>> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let file = File::open(path)
        .map_err(|_| crate::RuuterError::Config(format!("Cannot open constants file: {}", path)))?;

    let reader = BufReader::new(file);
    let mut constants = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Section headers ([DSL], etc.) are accepted for compatibility with
        // the original Java constants.ini format but do not scope keys —
        // DSL references keys flat as `[#KEY]`.
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            continue;
        }

        if let Some(pos) = trimmed.find('=') {
            let key = trimmed[..pos].trim();
            let value = trimmed[pos + 1..].trim();
            constants.insert(key.to_string(), value.to_string());
        }
    }

    Ok(constants)
}

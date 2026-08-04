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

    /// Audit finding 12 — response wrapper opt-in. When true and
    /// the DSL's terminating `return:` step didn't set `wrapper:
    /// false`, the response body is wrapped in
    /// `{"response": <value>}` (Java-parity `RuuterResponse` shape).
    /// Default `false` preserves the current Rust behaviour
    /// (unwrapped raw body). A ReturnStep's explicit `wrapper:
    /// true|false` always wins over this config.
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
/// for the wrapper opt-in semantics; other fields (dsl-with-response
/// / -without-response status codes) mirror Java's `finalResponse`
/// block from `application.yml` (finding 13).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ResponseConfig {
    /// Default wrapper mode when the ReturnStep doesn't specify.
    /// `false` = raw body (current Rust behaviour). `true` = wrap
    /// in `{"response": <value>}` (Java parity).
    #[serde(default)]
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
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LoggingConfig {
    #[serde(default)]
    pub display_request_content: bool,

    #[serde(default)]
    pub display_response_content: bool,

    #[serde(default)]
    pub print_stack_trace: bool,

    #[serde(default)]
    pub meaningful_errors: bool,
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

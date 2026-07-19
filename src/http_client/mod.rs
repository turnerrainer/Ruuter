use crate::config::AppConfig;
use crate::{Result, RuuterError};
use async_trait::async_trait;
use futures::StreamExt;
use once_cell::sync::OnceCell;
use reqwest::{Client, Method};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Task 044 — implemented by the framework's DSL router so
/// `HttpClient` can dispatch `http.<verb>` self-calls back through
/// the router in-process instead of over the network. Lives on the
/// `http_client` module (not `router`) to avoid a `router → http_client
/// → router` cyclic dependency at the type level.
///
/// The router-side implementation preserves guards, CSRF, path-param
/// resolution, and the OpenAPI-visible response shape — a self-call
/// is byte-identical to a loopback TCP call, just faster.
#[async_trait]
pub trait SelfCallHandler: Send + Sync {
    async fn execute_by_url(
        &self,
        method: &str,
        url_path: &str,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        body: HashMap<String, Value>,
    ) -> Result<HttpResponse>;
}

/// Task 044 — set of "URLs that are actually us." Compared against
/// each outbound `http.<verb>` URL; a match fires the short-circuit
/// dispatch through the [`SelfCallHandler`].
#[derive(Debug, Default, Clone)]
pub struct SelfOrigins {
    /// TCP endpoints — `(host, port)` pairs. Includes every literal
    /// listener bind plus the well-known localhost/127.0.0.1
    /// synonyms so DSLs that write `http://localhost:8080/…` still
    /// match a listener bound to `0.0.0.0`.
    pub tcp: HashSet<(String, u16)>,
}

impl SelfOrigins {
    /// Build the set from an AppConfig at boot. Never partial: if
    /// nothing is configured we still know the default `0.0.0.0:port`
    /// listener, so a DSL that writes `http://localhost:<port>/…`
    /// short-circuits correctly out of the box.
    pub fn from_config(config: &AppConfig) -> Self {
        let mut tcp: HashSet<(String, u16)> = HashSet::new();
        // The listener is 0.0.0.0:<port>. Register every loopback
        // synonym on that port so DSLs that write any of them all
        // resolve to the short-circuit path.
        for h in ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "::1"] {
            tcp.insert((h.to_string(), config.port));
        }
        Self { tcp }
    }

    /// True when `url` targets one of our own listeners.
    pub fn matches(&self, url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return false;
        }
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let port = match parsed.port() {
            Some(p) => p,
            None => match parsed.scheme() {
                "http" => 80,
                "https" => 443,
                _ => return false,
            },
        };
        self.tcp.contains(&(host.to_string(), port))
    }
}

/// Parse "host:port" (or "[::1]:port"). Currently unused — the
/// SelfOrigins detector only reads `config.port` since the multi-
/// listener path landed on a separate feature branch (task 043).
/// Kept here so the merge of 043 → dev doesn't have to re-add it.
#[allow(dead_code)]
fn split_host_port(s: &str) -> Option<(String, u16)> {
    // Handles "host:port" and IPv6 "[::1]:port"; anything else → None.
    if let Some(stripped) = s.strip_prefix('[') {
        // IPv6 form
        let end = stripped.find(']')?;
        let host = &stripped[..end];
        let rest = &stripped[end + 1..];
        let port = rest.strip_prefix(':')?.parse::<u16>().ok()?;
        Some((format!("[{}]", host), port))
    } else {
        let (h, p) = s.rsplit_once(':')?;
        Some((h.to_string(), p.parse::<u16>().ok()?))
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    pub default_timeout: Duration,
    /// Upper bound on the size of an upstream response body. Requests
    /// exceeding this are aborted mid-read and the caller sees an error
    /// instead of an OOM. `None` = unbounded (matches Java behavior).
    response_size_limit: Option<usize>,
    /// Whitelist of upstream response statuses. Empty = accept everything.
    status_allow_list: Vec<u16>,
    /// Outbound-call posture. If `disabled`, every request errors immediately.
    /// If `allowed_urls` is non-empty, the URL must match one of the prefixes.
    /// If `allowed_ips` is non-empty, the URL's host must be a bare IP in the
    /// list (host-name resolution is deliberately not performed — that would
    /// need DNS-rebinding protection to be safe).
    outbound_disabled: bool,
    allowed_url_prefixes: Vec<String>,
    allowed_ip_hosts: Vec<String>,
    /// Task 044 — set of hosts+ports that are actually our own
    /// listener. When a target URL matches, the request short-
    /// circuits into the router in-process.
    self_origins: Arc<SelfOrigins>,
    /// Task 044 — router handle. Filled AFTER the router is built
    /// (via `set_self_call_handler`), so the http_client → router →
    /// http_client cycle never actually exists at construction time.
    /// `OnceCell` inside `Arc` so every clone of this client sees
    /// the same one-shot slot.
    router_handle: Arc<OnceCell<Arc<dyn SelfCallHandler>>>,
}

impl HttpClient {
    pub fn new(config: &AppConfig) -> Self {
        let default_timeout = Duration::from_millis(config.http_request_timeout);
        let client = Client::builder()
            .timeout(default_timeout)
            .build()
            .unwrap();

        Self {
            client,
            default_timeout,
            response_size_limit: config.http_response_size_limit,
            status_allow_list: config.http_codes_allow_list.clone(),
            outbound_disabled: config.internal_requests.disabled,
            allowed_url_prefixes: config.internal_requests.allowed_urls.clone(),
            allowed_ip_hosts: config.internal_requests.allowed_ips.clone(),
            self_origins: Arc::new(SelfOrigins::from_config(config)),
            router_handle: Arc::new(OnceCell::new()),
        }
    }

    /// Task 044 — one-shot wiring: register the router as the
    /// SelfCallHandler this client will dispatch to when an outbound
    /// URL matches a self-origin. Called from `main.rs` after the
    /// router is constructed. Returns `Err` (silently, via log) if
    /// called twice — first-write-wins semantics via OnceCell.
    pub fn set_self_call_handler(&self, handler: Arc<dyn SelfCallHandler>) {
        if self.router_handle.set(handler).is_err() {
            tracing::warn!("HttpClient::set_self_call_handler called twice — ignoring second");
        }
    }

    /// Test-only builder: install a bare SelfOrigins for the client.
    /// Used by self-call tests that don't want to spin up a full
    /// AppConfig just to configure a listener.
    pub fn with_self_origins(mut self, origins: SelfOrigins) -> Self {
        self.self_origins = Arc::new(origins);
        self
    }

    /// Bare constructor for tests / callers that don't want to build a full
    /// AppConfig just to spin up an HttpClient.
    pub fn with_timeout_ms(default_timeout_ms: u64) -> Self {
        let default_timeout = Duration::from_millis(default_timeout_ms);
        let client = Client::builder()
            .timeout(default_timeout)
            .build()
            .unwrap();

        Self {
            client,
            default_timeout,
            response_size_limit: None,
            status_allow_list: Vec::new(),
            outbound_disabled: false,
            allowed_url_prefixes: Vec::new(),
            allowed_ip_hosts: Vec::new(),
            self_origins: Arc::new(SelfOrigins::default()),
            router_handle: Arc::new(OnceCell::new()),
        }
    }

    fn check_ssrf(&self, url: &str) -> Result<()> {
        if self.outbound_disabled {
            return Err(RuuterError::HttpRequest(
                "outbound HTTP is disabled by internal_requests.disabled".into(),
            ));
        }
        if !self.allowed_url_prefixes.is_empty()
            && !self
                .allowed_url_prefixes
                .iter()
                .any(|prefix| url.starts_with(prefix))
        {
            return Err(RuuterError::HttpRequest(format!(
                "url not in internal_requests.allowed_urls: {}",
                url
            )));
        }
        if !self.allowed_ip_hosts.is_empty() {
            let parsed = url::Url::parse(url)
                .map_err(|e| RuuterError::HttpRequest(format!("invalid url '{}': {}", url, e)))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| RuuterError::HttpRequest(format!("url has no host: {}", url)))?;
            if !self.allowed_ip_hosts.iter().any(|ip| ip == host) {
                return Err(RuuterError::HttpRequest(format!(
                    "url host '{}' not in internal_requests.allowed_ips",
                    host
                )));
            }
        }
        Ok(())
    }

    pub async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        query: Option<&HashMap<String, Value>>,
        headers: Option<&HashMap<String, Value>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        // Test-only URL rewriting (see `rewrite_url_for_tests`). Applied
        // BEFORE SSRF checks so tests can point at a local mock without
        // punching a hole in the allowlist.
        let rewritten = rewrite_url_for_tests(url);
        let url = rewritten.as_deref().unwrap_or(url);

        // Task 044 dispatch: if this URL matches one of our own
        // listeners AND a router handle was wired at boot, short-
        // circuit into the router instead of going over reqwest.
        // Semantic parity: guards run, path-params resolve, response
        // shape identical to a network-loopback call.
        //
        // If the handle isn't wired (bare `HttpClient::with_timeout_ms`
        // in a test, say), fall through to the normal path — matching
        // a self-origin isn't a failure, just a missed optimisation.
        if self.self_origins.matches(url) {
            if let Some(handler) = self.router_handle.get() {
                return self
                    .self_call_dispatch(handler.as_ref(), method, url, body, query, headers)
                    .await;
            }
        }

        self.check_ssrf(url)?;

        let mut request = self.client
            .request(method, url)
            .timeout(timeout.unwrap_or(self.default_timeout));

        if let Some(q) = query {
            for (k, v) in q {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                request = request.query(&[(k.as_str(), s)]);
            }
        }

        if let Some(h) = headers {
            for (k, v) in h {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                request = request.header(k.as_str(), s);
            }
        }

        if let Some(b) = body {
            request = request.json(b);
        }

        let response = request.send().await?;
        let status = response.status().as_u16();

        if !self.status_allow_list.is_empty()
            && !self.status_allow_list.contains(&status)
        {
            return Err(RuuterError::HttpRequest(format!(
                "upstream status {} not in http_codes_allow_list",
                status
            )));
        }

        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // Enforce response-body cap by streaming and tallying rather than
        // reading the whole body into memory. `Content-Length` is honored
        // upfront when the server sends it — chunked responses fall through
        // to the streaming check.
        if let Some(cap) = self.response_size_limit {
            if let Some(len_str) = response_headers.get("content-length") {
                if let Ok(declared) = len_str.parse::<usize>() {
                    if declared > cap {
                        return Err(RuuterError::HttpRequest(format!(
                            "upstream response body {} bytes exceeds http_response_size_limit {}",
                            declared, cap
                        )));
                    }
                }
            }
        }

        let bytes = if let Some(cap) = self.response_size_limit {
            let mut stream = response.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    RuuterError::HttpRequest(format!("upstream read error: {}", e))
                })?;
                if buf.len() + chunk.len() > cap {
                    return Err(RuuterError::HttpRequest(format!(
                        "upstream response body exceeded http_response_size_limit {}",
                        cap
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            buf
        } else {
            response
                .bytes()
                .await
                .map_err(|e| RuuterError::HttpRequest(format!("upstream read error: {}", e)))?
                .to_vec()
        };

        let body: Option<Value> = if bytes.is_empty() {
            None
        } else {
            serde_json::from_slice::<Value>(&bytes).ok()
        };

        Ok(HttpResponse {
            status,
            body,
            headers: response_headers,
        })
    }

    /// Task 044 — invoke the router in-process for a URL whose
    /// origin matches one of our own listeners. Result is packaged
    /// into the same `HttpResponse` shape a network call would
    /// produce so DSL callers can't tell the difference.
    async fn self_call_dispatch(
        &self,
        handler: &dyn SelfCallHandler,
        method: Method,
        url: &str,
        body: Option<&Value>,
        query: Option<&HashMap<String, Value>>,
        headers: Option<&HashMap<String, Value>>,
    ) -> Result<HttpResponse> {
        let parsed = url::Url::parse(url).map_err(|e| {
            RuuterError::HttpRequest(format!("invalid self-call url '{}': {}", url, e))
        })?;
        let path = parsed.path().to_string();

        // Merge URL query params with any explicit `query` argument.
        // execute_dsl consumes a single flat map; DSL-side URL parsing
        // (executor's `${incoming.query.foo}` bindings) sees both.
        let mut merged_query: HashMap<String, Value> = HashMap::new();
        for (k, v) in parsed.query_pairs() {
            merged_query.insert(k.to_string(), Value::String(v.to_string()));
        }
        if let Some(q) = query {
            for (k, v) in q {
                merged_query.insert(k.clone(), v.clone());
            }
        }

        // Header map: normalise values to String. Downstream DslRouter
        // takes HashMap<String, String>.
        let mut header_strings: HashMap<String, String> = HashMap::new();
        if let Some(h) = headers {
            for (k, v) in h {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                header_strings.insert(k.to_lowercase(), s);
            }
        }

        // Body: HashMap<String, Value>. Non-object bodies (bare
        // numbers, arrays) are rare on inbound HTTP but shouldn't
        // silently drop — wrap under a `_body` key so DSLs that
        // expect that structure can access them.
        let body_map: HashMap<String, Value> = match body {
            Some(Value::Object(map)) => map
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            Some(other) => {
                let mut m = HashMap::new();
                m.insert("_body".to_string(), other.clone());
                m
            }
            None => HashMap::new(),
        };

        let resp = handler
            .execute_by_url(method.as_str(), &path, merged_query, header_strings, body_map)
            .await?;
        // Same status/size gates as the network path (no size gate
        // on self-calls today — no wire transfer to cap; file a
        // follow-up if a DSL author needs it).
        if !self.status_allow_list.is_empty() && !self.status_allow_list.contains(&resp.status) {
            return Err(RuuterError::HttpRequest(format!(
                "self-call status {} not in http_codes_allow_list",
                resp.status
            )));
        }
        Ok(resp)
    }
}

/// Test-only URL rewriting. When the env var
/// `RUUTER_HTTP_REWRITE` is set to a comma-separated list of
/// `from=to` pairs, any outbound URL whose origin matches a `from`
/// origin has its origin replaced with the corresponding `to`. Path,
/// query, and headers are preserved.
///
/// Example: `RUUTER_HTTP_REWRITE=https://jsonplaceholder.typicode.com=http://127.0.0.1:9999`
///
/// Off by default (env var absent → no rewriting). Kept out of the
/// `HttpClient` struct so no test-mode flag propagates into production
/// config surfaces.
fn rewrite_url_for_tests(url: &str) -> Option<String> {
    let raw = std::env::var("RUUTER_HTTP_REWRITE").ok()?;
    if raw.is_empty() {
        return None;
    }
    for pair in raw.split(',') {
        let mut parts = pair.splitn(2, '=');
        let from = parts.next()?.trim();
        let to = parts.next()?.trim();
        if from.is_empty() || to.is_empty() {
            continue;
        }
        if let Some(rest) = url.strip_prefix(from) {
            return Some(format!("{}{}", to, rest));
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub headers: HashMap<String, String>,
}

use crate::config::AppConfig;
use crate::{Result, RuuterError};
use futures::StreamExt;
use reqwest::{Client, Method};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod uds;

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
    /// Task 043 — host → UDS socket path aliases. When an outbound
    /// URL's host matches a key here, the request goes over the
    /// mapped Unix socket instead of TCP. Shared via `Arc` so clones
    /// don't duplicate the map.
    unix_socket_map: Arc<HashMap<String, PathBuf>>,
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
            unix_socket_map: Arc::new(config.unix_socket_map.clone()),
        }
    }

    /// Test-only builder: replaces the UDS alias map. Returns a fresh
    /// client rather than mutating in place because the field is
    /// `Arc`-shared with existing clones.
    pub fn with_unix_socket_map(mut self, map: HashMap<String, PathBuf>) -> Self {
        self.unix_socket_map = Arc::new(map);
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
            unix_socket_map: Arc::new(HashMap::new()),
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

        // Task 043 dispatch: explicit `unix://` scheme, or a
        // TCP-shaped URL whose host is an alias for a Unix socket.
        // SSRF allow-lists apply to TCP-shaped URLs; UDS bypasses them
        // (a Unix path is definitionally not a remote address).
        if url.starts_with("unix://") {
            return self.uds_request_from_unix_url(url, method, body, query, headers, timeout).await;
        }
        if let Some(sock_path) = self.host_alias_lookup(url) {
            return self.uds_request_from_alias(&sock_path, url, method, body, query, headers, timeout).await;
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

    /// If `url` is a TCP-shaped URL (`http://host/...`) whose host
    /// matches a `unix_socket_map` alias, return the socket path.
    /// `None` otherwise — caller should proceed with the TCP path.
    fn host_alias_lookup(&self, url: &str) -> Option<PathBuf> {
        if self.unix_socket_map.is_empty() {
            return None;
        }
        // Parse just enough to extract the host; a full url::Url parse
        // rejects some quirks (missing path) that we want to allow.
        let parsed = url::Url::parse(url).ok()?;
        let host = parsed.host_str()?;
        self.unix_socket_map.get(host).cloned()
    }

    async fn uds_request_from_unix_url(
        &self,
        url: &str,
        method: Method,
        body: Option<&Value>,
        query: Option<&HashMap<String, Value>>,
        headers: Option<&HashMap<String, Value>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        let (socket_path, mut path_and_query) = uds::parse_unix_url(url)?;
        // Append query params if the caller passed any and the URL
        // didn't already have a query string.
        path_and_query = merge_query_params(&path_and_query, query);
        let resp = uds::request_over_unix(
            &socket_path,
            "localhost",
            &path_and_query,
            hyper_method(method),
            body,
            headers,
            timeout.unwrap_or(self.default_timeout),
        )
        .await?;
        self.enforce_status_and_size(&resp)?;
        Ok(resp)
    }

    async fn uds_request_from_alias(
        &self,
        socket_path: &std::path::Path,
        url: &str,
        method: Method,
        body: Option<&Value>,
        query: Option<&HashMap<String, Value>>,
        headers: Option<&HashMap<String, Value>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        let parsed = url::Url::parse(url).map_err(|e| {
            RuuterError::HttpRequest(format!("invalid aliased url '{}': {}", url, e))
        })?;
        let origin_host = parsed.host_str().unwrap_or("localhost").to_string();
        // Combine URL's own path+query with any extra `query` argument
        let base = if let Some(q) = parsed.query() {
            format!("{}?{}", parsed.path(), q)
        } else {
            parsed.path().to_string()
        };
        let path_and_query = merge_query_params(&base, query);
        let resp = uds::request_over_unix(
            socket_path,
            &origin_host,
            &path_and_query,
            hyper_method(method),
            body,
            headers,
            timeout.unwrap_or(self.default_timeout),
        )
        .await?;
        self.enforce_status_and_size(&resp)?;
        Ok(resp)
    }

    fn enforce_status_and_size(&self, resp: &HttpResponse) -> Result<()> {
        if !self.status_allow_list.is_empty() && !self.status_allow_list.contains(&resp.status) {
            return Err(RuuterError::HttpRequest(format!(
                "upstream status {} not in http_codes_allow_list",
                resp.status
            )));
        }
        if let Some(cap) = self.response_size_limit {
            // UDS path reads the full body via `.collect()` — we can
            // only enforce the cap post-hoc. For streaming UDS with
            // mid-read abort, see follow-up task.
            let approx = resp
                .body
                .as_ref()
                .map(|v| serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0))
                .unwrap_or(0);
            if approx > cap {
                return Err(RuuterError::HttpRequest(format!(
                    "uds upstream body {} bytes exceeds http_response_size_limit {}",
                    approx, cap
                )));
            }
        }
        Ok(())
    }
}

fn hyper_method(m: Method) -> http::Method {
    // reqwest::Method and http::Method are the same underlying crate;
    // we're just re-exporting through different names. Since 0.12,
    // reqwest::Method IS http::Method (re-export). Direct pass through.
    m
}

fn merge_query_params(path_and_query: &str, extra: Option<&HashMap<String, Value>>) -> String {
    let Some(q) = extra else {
        return path_and_query.to_string();
    };
    if q.is_empty() {
        return path_and_query.to_string();
    }
    let sep = if path_and_query.contains('?') { '&' } else { '?' };
    let parts: Vec<String> = q
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{}={}", urlencoding_encode(k), urlencoding_encode(&s))
        })
        .collect();
    format!("{}{}{}", path_and_query, sep, parts.join("&"))
}

fn urlencoding_encode(s: &str) -> String {
    // Minimal %-encoding for query values: encode reserved chars
    // sufficient for URL query strings. Full encoder isn't required
    // because typical UDS callees are internal and see simple values.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
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

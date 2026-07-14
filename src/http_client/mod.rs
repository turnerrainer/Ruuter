use crate::config::AppConfig;
use crate::{Result, RuuterError};
use futures::StreamExt;
use reqwest::{Client, Method};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

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
        }
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
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub headers: HashMap<String, String>,
}

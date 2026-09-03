//! Task 050 — UDS keep-alive connection pool.
//!
//! Replaces task 043's per-request handshake with a pooled client
//! built on `hyper-util::client::legacy::Client`. Per unique socket
//! path, one `Client` instance is cached; every `Client` maintains
//! an internal pool of keep-alive connections to that socket. Idle
//! connections are reused on the next request; the first request
//! after downstream restart sees a failure and retries against a
//! fresh connection.
//!
//! Design: one `Client<UdsConnector, Full<Bytes>>` per unique
//! socket path, cached in a `DashMap`. Each Connector holds only
//! its target socket path (a Uri-agnostic operation because
//! `UnixStream::connect` doesn't care about URL semantics).
//! Requests go through `client.request(req)` which handles pool
//! checkout, request dispatch, and pool return.

use crate::{Result, RuuterError};
use dashmap::DashMap;
use http::{Method, Request, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::UnixStream;

use super::HttpResponse;

/// Connector that always connects to a fixed Unix socket path,
/// regardless of the `Uri` passed in. The Uri is used by hyper for
/// the request-line target + Host header; the connector only cares
/// about the transport.
///
/// Cloneable because hyper-util's Client wraps it internally and
/// calls `clone()` on connection checkout.
#[derive(Clone)]
struct UdsConnector {
    path: Arc<PathBuf>,
}

impl tower::Service<Uri> for UdsConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = self.path.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(&*path).await?;
            Ok(TokioIo::new(stream))
        })
    }
}

/// HTTP protocol version this pool will speak to its target sockets.
/// One pool = one version; requests never negotiate. Configured at
/// construction from `AppConfig::uds_http_version`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UdsHttpVersion {
    #[default]
    Http1,
    /// h2c — HTTP/2 cleartext (task 049). Multiplexes streams on a
    /// single connection, eliminating HOL blocking. Requires the
    /// target sidecar to speak h2c on the same socket.
    Http2,
}

impl From<crate::config::HttpVersion> for UdsHttpVersion {
    fn from(v: crate::config::HttpVersion) -> Self {
        match v {
            crate::config::HttpVersion::Http1 => UdsHttpVersion::Http1,
            crate::config::HttpVersion::Http2 => UdsHttpVersion::Http2,
        }
    }
}

type PooledUdsClient = Arc<Client<UdsConnector, Full<Bytes>>>;
type UdsClientMap = Arc<DashMap<PathBuf, PooledUdsClient>>;

/// Pool of pooled UDS clients — one Client per unique socket path.
///
/// Cloneable because it's just an `Arc<DashMap>` inside. Every
/// clone of `HttpClient` shares the same pool.
#[derive(Clone)]
pub struct UdsPool {
    inner: UdsClientMap,
    idle_timeout: Duration,
    max_idle_per_host: usize,
    http_version: UdsHttpVersion,
}

impl UdsPool {
    pub fn new(idle_timeout: Duration, max_idle_per_host: usize) -> Self {
        Self::with_version(idle_timeout, max_idle_per_host, UdsHttpVersion::Http1)
    }

    pub fn with_version(
        idle_timeout: Duration,
        max_idle_per_host: usize,
        http_version: UdsHttpVersion,
    ) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            idle_timeout,
            max_idle_per_host,
            http_version,
        }
    }

    pub fn http_version(&self) -> UdsHttpVersion {
        self.http_version
    }

    /// Get (or lazily create) the pooled Client for `socket_path`.
    ///
    /// First call for a new socket path builds a Client with a fresh
    /// UdsConnector. Subsequent calls return the cached Arc<Client>,
    /// which internally owns the connection pool for that socket.
    /// The client's HTTP version is fixed at pool construction —
    /// mixing h1 and h2 to the same socket needs two pools.
    fn client_for(&self, socket_path: &Path) -> Arc<Client<UdsConnector, Full<Bytes>>> {
        let key = socket_path.to_path_buf();
        if let Some(existing) = self.inner.get(&key) {
            return existing.clone();
        }
        // Race: two threads may build simultaneously; DashMap's
        // `entry` API serialises the closure. The loser's Client is
        // dropped harmlessly (no connections opened yet).
        self.inner
            .entry(key.clone())
            .or_insert_with(|| {
                let connector = UdsConnector {
                    path: Arc::new(key.clone()),
                };
                let mut builder = hyper_util::client::legacy::Builder::new(TokioExecutor::new());
                builder
                    .pool_idle_timeout(self.idle_timeout)
                    .pool_max_idle_per_host(self.max_idle_per_host);
                if self.http_version == UdsHttpVersion::Http2 {
                    builder.http2_only(true);
                }
                let client = builder.build::<_, Full<Bytes>>(connector);
                Arc::new(client)
            })
            .clone()
    }

    /// Number of unique socket paths with a cached client. Used by
    /// tests to assert pool structure.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for UdsPool {
    fn default() -> Self {
        Self::with_version(Duration::from_secs(30), 32, UdsHttpVersion::Http1)
    }
}

/// Pool-backed replacement for the per-request handshake in
/// `super::uds::request_over_unix`. Same signature (plus `pool`)
/// and produces the same `HttpResponse` shape.
///
/// Argument count intentionally mirrors the underlying reqwest-shape
/// (socket path, host, path, method, body, headers, timeout) — folding
/// them into a struct would just move the plumbing without simplifying
/// the call sites that already have each value in scope.
#[allow(clippy::too_many_arguments)]
pub async fn request_over_unix_pooled(
    pool: &UdsPool,
    socket_path: &Path,
    origin_host: &str,
    path_and_query: &str,
    method: Method,
    body: Option<&Value>,
    headers: Option<&HashMap<String, Value>>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let client = pool.client_for(socket_path);

    let body_bytes: Bytes = match body {
        Some(v) => serde_json::to_vec(v)
            .map_err(|e| RuuterError::HttpRequest(format!("serialise body: {}", e)))?
            .into(),
        None => Bytes::new(),
    };

    // hyper's Client wants an absolute URI. We fabricate a scheme+
    // authority; the UdsConnector ignores them because it always
    // connects to the fixed socket. The path+query part IS honoured
    // by hyper for the request line, and the authority becomes the
    // Host header default (which we override below).
    let uri: Uri = format!("http://{}{}", origin_host, path_and_query)
        .parse()
        .map_err(|e| RuuterError::HttpRequest(format!("build uds uri: {}", e)))?;

    let mut req_builder = Request::builder().method(method).uri(uri);

    let has_content_length = headers
        .map(|h| h.keys().any(|k| k.eq_ignore_ascii_case("content-length")))
        .unwrap_or(false);
    let has_content_type = headers
        .map(|h| h.keys().any(|k| k.eq_ignore_ascii_case("content-type")))
        .unwrap_or(false);

    if let Some(h) = headers {
        for (k, v) in h {
            // Issue #57 — parity with `http_client/mod.rs` and
            // `uds.rs`: null-valued headers drop rather than emit
            // `X-Foo: null`.
            if matches!(v, Value::Null) {
                continue;
            }
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            req_builder = req_builder.header(k.as_str(), s);
        }
    }
    if !body_bytes.is_empty() && !has_content_length {
        req_builder = req_builder.header("content-length", body_bytes.len());
    }
    if !body_bytes.is_empty() && !has_content_type {
        req_builder = req_builder.header("content-type", "application/json");
    }

    let req = req_builder
        .body(Full::new(body_bytes))
        .map_err(|e| RuuterError::HttpRequest(format!("build uds request: {}", e)))?;

    let fut = client.request(req);
    let res = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| RuuterError::Timeout(format!("uds request exceeded {:?}", timeout)))?
        .map_err(|e| RuuterError::HttpRequest(format!("uds send (pooled): {}", e)))?;

    let status = res.status().as_u16();
    let response_headers: HashMap<String, String> = res
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let bytes = tokio::time::timeout(timeout, res.into_body().collect())
        .await
        .map_err(|_| RuuterError::Timeout(format!("uds body exceeded {:?}", timeout)))?
        .map_err(|e| RuuterError::HttpRequest(format!("uds body (pooled): {}", e)))?
        .to_bytes();

    let parsed_body: Option<Value> = if bytes.is_empty() {
        None
    } else {
        serde_json::from_slice::<Value>(&bytes).ok()
    };

    Ok(HttpResponse {
        status,
        body: parsed_body,
        headers: response_headers,
    })
}

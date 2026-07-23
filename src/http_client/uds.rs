//! Task 043 — outbound Unix-domain-socket transport.
//!
//! Small hyper-based HTTP/1.1 client that dispatches over a
//! `tokio::net::UnixStream` instead of TCP. Used by
//! [`super::HttpClient`] when a URL is either:
//!
//! - explicit UDS: `unix:///var/run/foo.sock/some/path?q=1`, or
//! - a host that matches a configured `unix_socket_map` alias
//!   (`http://resql/query` where `resql` maps to a socket path).
//!
//! Both forms produce a request-line path of `/some/path?q=1`
//! and connect to the mapped socket. Response parsing produces the
//! same [`super::HttpResponse`] shape as the TCP path, so DSL
//! authors see identical semantics.

use crate::{Result, RuuterError};
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::net::UnixStream;

use super::HttpResponse;

/// Perform one HTTP/1.1 request over a Unix-domain socket.
///
/// - `socket_path` — absolute path to the target UDS
/// - `origin_host` — value to send as `Host:` header (hyper won't
///   default one for us; UDS URLs have no meaningful host, but many
///   upstream services expect the header)
/// - `path_and_query` — the request-line target, e.g. `/orders?limit=10`
/// - `method`, `body`, `headers` — same shape as TCP client
/// - `timeout` — wall-clock limit; both connect and read are covered
pub async fn request_over_unix(
    socket_path: &Path,
    origin_host: &str,
    path_and_query: &str,
    method: Method,
    body: Option<&Value>,
    headers: Option<&HashMap<String, Value>>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let socket_path = socket_path.to_path_buf();
    let origin_host = origin_host.to_string();
    let path_and_query = path_and_query.to_string();
    let body_bytes: Bytes = match body {
        Some(v) => serde_json::to_vec(v)
            .map_err(|e| RuuterError::HttpRequest(format!("serialise body: {}", e)))?
            .into(),
        None => Bytes::new(),
    };
    let hdr_owned: Vec<(String, String)> = headers
        .map(|h| {
            h.iter()
                .map(|(k, v)| {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect()
        })
        .unwrap_or_default();

    let fut = async move {
        // Connect
        let stream = UnixStream::connect(&socket_path).await.map_err(|e| {
            RuuterError::HttpRequest(format!("unix connect {}: {}", socket_path.display(), e))
        })?;

        // Handshake — hyper's low-level client API for HTTP/1
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|e| RuuterError::HttpRequest(format!("uds handshake: {}", e)))?;

        // Drive the connection I/O to completion on a spawned task.
        // Terminates when `sender` is dropped and the server closes.
        tokio::spawn(async move {
            let _ = conn.await;
        });

        // Build the request
        let mut req_builder = Request::builder()
            .method(method)
            .uri(&path_and_query)
            .header("host", &origin_host);

        let has_content_length = hdr_owned
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
        let has_content_type = hdr_owned
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));

        for (k, v) in &hdr_owned {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }
        if !body_bytes.is_empty() && !has_content_length {
            req_builder = req_builder.header("content-length", body_bytes.len());
        }
        if !body_bytes.is_empty() && !has_content_type {
            req_builder = req_builder.header("content-type", "application/json");
        }

        let req = req_builder
            .body(Full::new(body_bytes.clone()))
            .map_err(|e| RuuterError::HttpRequest(format!("build request: {}", e)))?;

        // Send + collect response
        let res = sender
            .send_request(req)
            .await
            .map_err(|e| RuuterError::HttpRequest(format!("uds send: {}", e)))?;

        let status = res.status().as_u16();
        let response_headers: HashMap<String, String> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let body_bytes = res
            .into_body()
            .collect()
            .await
            .map_err(|e| RuuterError::HttpRequest(format!("uds body: {}", e)))?
            .to_bytes();

        let parsed_body: Option<Value> = if body_bytes.is_empty() {
            None
        } else {
            serde_json::from_slice::<Value>(&body_bytes).ok()
        };

        Ok::<HttpResponse, RuuterError>(HttpResponse {
            status,
            body: parsed_body,
            headers: response_headers,
        })
    };

    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(RuuterError::Timeout(format!(
            "uds request exceeded {:?}",
            timeout
        ))),
    }
}

/// Parse `unix:///abs/socket/path/rest?q=1` → (socket_path, "/rest?q=1").
///
/// The URL format is: `unix://` + `<absolute-socket-path>` + `<request-target>`.
/// We split at the *last* `/` that follows a `.sock` (or `.socket`) segment
/// so callers can reference arbitrary paths under a socket without
/// ambiguity. When neither `.sock` nor `.socket` appears, we fall back
/// to splitting after the FIRST path segment past the leading `///`.
///
/// Returns `Err` if the URL doesn't parse as `unix://` or has no
/// socket path segment.
pub fn parse_unix_url(url: &str) -> Result<(std::path::PathBuf, String)> {
    let rest = url
        .strip_prefix("unix://")
        .ok_or_else(|| RuuterError::HttpRequest(format!("not a unix:// URL: {}", url)))?;
    if !rest.starts_with('/') {
        return Err(RuuterError::HttpRequest(format!(
            "unix:// URL missing leading / after scheme: {}",
            url
        )));
    }

    // Try `.socket` before `.sock` — otherwise `.sock` matches
    // inside `.socket` (it's a prefix) and truncates the socket
    // path mid-name. This is exactly the class of ordering bug that
    // would silently corrupt every `.socket`-suffixed URL.
    for marker in [".socket", ".sock"] {
        if let Some(pos) = rest.find(marker) {
            let end = pos + marker.len();
            let (sock, tail) = rest.split_at(end);
            let path_and_query = if tail.is_empty() { "/" } else { tail };
            return Ok((std::path::PathBuf::from(sock), path_and_query.to_string()));
        }
    }

    // Fallback: assume the socket path ends at the first `/` past
    // position 1 (i.e. `unix:///a/b/c` → socket=`/a`, path=`/b/c`).
    // This is imprecise for real deployments; docs recommend the
    // `.sock` marker or the alias-map form.
    let after_leading = &rest[1..];
    match after_leading.find('/') {
        Some(idx) => {
            let (sock_rest, tail) = after_leading.split_at(idx);
            Ok((
                std::path::PathBuf::from(format!("/{}", sock_rest)),
                tail.to_string(),
            ))
        }
        None => Ok((std::path::PathBuf::from(rest), "/".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unix_url_with_sock_marker() {
        let (sock, path) = parse_unix_url("unix:///var/run/foo.sock/orders?limit=10").unwrap();
        assert_eq!(sock, std::path::PathBuf::from("/var/run/foo.sock"));
        assert_eq!(path, "/orders?limit=10");
    }

    #[test]
    fn parse_unix_url_socket_extension() {
        let (sock, path) = parse_unix_url("unix:///run/tim.socket/verify").unwrap();
        assert_eq!(sock, std::path::PathBuf::from("/run/tim.socket"));
        assert_eq!(path, "/verify");
    }

    #[test]
    fn parse_unix_url_root_path() {
        let (sock, path) = parse_unix_url("unix:///tmp/x.sock").unwrap();
        assert_eq!(sock, std::path::PathBuf::from("/tmp/x.sock"));
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_unix_url_fallback_no_sock_extension() {
        let (sock, path) = parse_unix_url("unix:///socket/rest").unwrap();
        assert_eq!(sock, std::path::PathBuf::from("/socket"));
        assert_eq!(path, "/rest");
    }

    #[test]
    fn parse_unix_url_rejects_non_unix_scheme() {
        assert!(parse_unix_url("http://example/foo").is_err());
    }

    #[test]
    fn parse_unix_url_rejects_missing_leading_slash() {
        assert!(parse_unix_url("unix://").is_err());
        assert!(parse_unix_url("unix://host/path").is_err());
    }
}

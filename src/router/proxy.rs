//! Issue #134 — pass-through proxy handler for routes declared with
//! `declaration.proxy:`.
//!
//! The handler is a dedicated code path that bypasses `StepEngine`
//! entirely. It streams the client's request body straight to the
//! configured upstream (no buffering), streams the upstream's response
//! back (no decoding), and preserves hop-by-hop semantics on both
//! legs. The design goal is byte-identical forwarding for workloads
//! like AS4 / eDelivery where request signatures cover the exact
//! bytes.
//!
//! The security floor Ruuter owns on every HTTP route still applies:
//! guards run (against a header-only `ExecutionContext` — bodies are
//! not parsed), SSRF is enforced on the upstream URL via
//! `HttpClient::check_ssrf`, size caps are enforced per-route (declared
//! via `declaration.proxy.max_body_bytes`) with a Content-Length
//! preflight AND a mid-stream byte counter, and a per-route semaphore
//! bounds concurrent in-flight proxied requests. See
//! `book/src/dsl/proxy.md` for the full contract.

use crate::config::AppConfig;
use crate::dsl::ProxyDeclaration;
use crate::http_client::{classify_transport_error, HttpClient, SsrfResolution};
use crate::{Result, RuuterError};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

/// RFC 7230 §6.1 — headers that MUST NOT be forwarded by an
/// intermediary. Compared case-insensitively. `host` is also stripped
/// on the request leg (reqwest sets its own from the upstream URL);
/// `expect` is stripped because 100-continue is handled at the hyper
/// layer and relaying it introduces race conditions outside the
/// scope of this PR (see `book/src/dsl/proxy.md` "Known caveats").
const HOP_BY_HOP_REQUEST: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "expect",
];

/// Response-leg hop-by-hop list. Same RFC §6.1 set minus `host` /
/// `expect` (irrelevant on responses).
const HOP_BY_HOP_RESPONSE: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Dedicated reqwest client for pass-through proxy routes plus the
/// per-route Semaphore registry. Owned by `DslRouter`; cheap to clone
/// (reqwest::Client is `Arc` inside; DashMap sits behind `Arc`).
#[derive(Clone)]
pub struct ProxyClient {
    client: reqwest::Client,
    http_client: HttpClient,
    semaphores: Arc<DashMap<String, Arc<Semaphore>>>,
    response_size_limit: Option<usize>,
}

impl ProxyClient {
    /// Build a non-decompressing pooled reqwest client sized per
    /// `pass_through_proxy:` config. Pool is independent of the
    /// `http.*` step's pool so a saturated proxy workload cannot
    /// starve normal outbound traffic.
    pub fn new(config: &AppConfig, http_client: HttpClient) -> Result<Self> {
        let cfg = &config.pass_through_proxy;
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(cfg.pool_max_idle_per_host)
            .pool_idle_timeout(Duration::from_millis(cfg.pool_idle_timeout_ms))
            .connect_timeout(Duration::from_millis(cfg.connect_timeout_ms))
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| {
                RuuterError::HttpRequest(format!("proxy reqwest client build failed: {}", e))
            })?;
        Ok(Self {
            client,
            http_client,
            semaphores: Arc::new(DashMap::new()),
            response_size_limit: config.http_response_size_limit,
        })
    }

    fn semaphore_for_route(&self, key: &str, cap: Option<u32>) -> Option<Arc<Semaphore>> {
        let cap = cap?;
        let entry = self
            .semaphores
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(cap as usize)));
        Some(entry.clone())
    }

    /// Build a per-request reqwest client when SSRF resolved to pinned
    /// addresses. In the unpinned case we share the pooled client,
    /// which keeps keep-alive working for the common case.
    fn client_for(
        &self,
        ssrf: &SsrfResolution,
        decl: &ProxyDeclaration,
    ) -> Result<reqwest::Client> {
        match ssrf {
            SsrfResolution::NoPinning => Ok(self.client.clone()),
            SsrfResolution::Pinned { host, addrs } => {
                let cfg_timeout = decl
                    .request_timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(Duration::from_secs(60));
                let mut builder = reqwest::Client::builder()
                    .connect_timeout(cfg_timeout)
                    .no_gzip()
                    .no_brotli()
                    .no_deflate()
                    .redirect(reqwest::redirect::Policy::none());
                for addr in addrs {
                    builder = builder.resolve(host, *addr);
                }
                builder.build().map_err(|e| {
                    RuuterError::HttpRequest(format!(
                        "proxy DNS-pinned client build failed for '{}': {}",
                        host, e
                    ))
                })
            }
        }
    }

    /// Forward one request through this proxy route. Returns an axum
    /// response that is either (a) the streamed upstream response with
    /// hop-by-hop stripped on the response leg or (b) a structured
    /// error (413 / 415 / 502 / 503) when a Ruuter-owned gate rejects
    /// the request.
    pub async fn forward(
        &self,
        decl: &ProxyDeclaration,
        route_key: &str,
        traceparent: &str,
        request: axum::http::Request<Body>,
    ) -> Response<Body> {
        let (parts, body) = request.into_parts();

        // (1) Content-Length preflight. Reject BEFORE acquiring a
        // Semaphore slot so a flood of oversize declarations doesn't
        // tie up concurrency budget.
        if let Some(cl) = parts
            .headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            if cl > decl.max_body_bytes {
                return body_too_large_preflight(cl, decl.max_body_bytes);
            }
        }

        // (2) Content-Encoding allowlist. Comma-separated per RFC 7231
        // §3.1.2.2. Trimmed, lower-cased; `identity` implicit when the
        // header is absent (RFC default).
        let ce_raw = parts
            .headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("identity")
            .to_string();
        for entry in ce_raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let normal = entry.to_ascii_lowercase();
            if !decl
                .allowed_encodings
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&normal))
            {
                return unsupported_encoding(entry, &decl.allowed_encodings);
            }
        }

        // (3) SSRF — same gate as `http.*` steps (allowlist, private-
        // network block, DNS-rebinding close). An operator who put the
        // upstream host in `allowed_ips` / `allowed_urls` opts into
        // this origin explicitly.
        let ssrf = match self.http_client.check_ssrf(&decl.upstream).await {
            Ok(r) => r,
            Err(e) => return proxy_upstream_rejected(&e.to_string()),
        };

        // (4) Per-route Semaphore. Overflow is 503 + Retry-After: 1
        // (fail-fast instead of queueing — unbounded wait would turn a
        // slow upstream into a stampede on the next deploy's warm
        // phase). `_permit` is dropped at end of this function (or
        // return path), releasing the slot even on error.
        let permit = match self.semaphore_for_route(route_key, decl.max_in_flight) {
            Some(sem) => match sem.try_acquire_owned() {
                Ok(p) => Some(p),
                Err(_) => return capacity_exceeded(decl.max_in_flight.unwrap_or(0)),
            },
            None => None,
        };

        // (5) Build the upstream URL by appending the inbound query
        // string (if any) to the declaration's `upstream`. Transparent
        // proxy semantics: `?x=1&x=2` reaches the upstream verbatim —
        // last-wins resolution (§ T-32) is a DSL concern, not ours.
        let upstream_url = match merge_query(&decl.upstream, parts.uri.query()) {
            Ok(u) => u,
            Err(e) => return proxy_upstream_rejected(&format!("invalid upstream URL: {}", e)),
        };

        // (6) Pick client (pinned or shared pool).
        let effective_client = match self.client_for(&ssrf, decl) {
            Ok(c) => c,
            Err(e) => return proxy_upstream_rejected(&e.to_string()),
        };

        // (7) Convert axum::Method → reqwest::Method via byte parse so
        // non-standard verbs (RFC 7231 extensions) still forward.
        let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
            Ok(m) => m,
            Err(e) => return proxy_upstream_rejected(&format!("invalid method: {}", e)),
        };

        // (8) Request headers: strip hop-by-hop + Connection-named
        // dynamic hop-by-hop. Inject / forward traceparent. Host is
        // reqwest's job.
        let forwarded_headers =
            build_forward_request_headers(&parts.headers, decl.preserve_headers, traceparent);

        // (9) Body stream with mid-stream cap + idle-frame timeout.
        // The byte counter is atomic so the exact-breach boundary is
        // observable to tests, and the io::Error we emit on breach is
        // propagated by reqwest::Body::wrap_stream — the upstream
        // connection is aborted (hyper closes it mid-request).
        let body_stream = body.into_data_stream();
        let metered = metered_request_body(
            body_stream,
            decl.max_body_bytes,
            decl.inbound_progress_timeout_ms.map(Duration::from_millis),
        );
        let reqwest_body = reqwest::Body::wrap_stream(metered);

        // (10) Build + send with overall request timeout. Timeout
        // covers connect + send + first-byte-of-response; subsequent
        // response streaming is bounded by the response-size cap, not
        // this timeout.
        let mut req_builder = effective_client.request(method, &upstream_url);
        for (name, value) in forwarded_headers.iter() {
            req_builder = req_builder.header(name, value);
        }
        req_builder = req_builder.body(reqwest_body);

        let overall = decl.request_timeout_ms.map(Duration::from_millis);
        let send_fut = req_builder.send();
        let send_result = match overall {
            Some(t) => match timeout(t, send_fut).await {
                Ok(r) => r,
                Err(_) => {
                    drop(permit);
                    return transport_error_response("timeout", "upstream deadline exceeded");
                }
            },
            None => send_fut.await,
        };

        let upstream = match send_result {
            Ok(r) => r,
            Err(e) => {
                drop(permit);
                let kind = classify_transport_error(&e);
                return transport_error_response(&kind, &e.to_string());
            }
        };

        // (11) Response headers: strip hop-by-hop. Status + headers
        // pass through verbatim otherwise (including upstream 4xx /
        // 5xx — a proxy must not re-map legitimate upstream status
        // codes).
        let status = upstream.status();
        let response_headers = strip_response_hop_by_hop(upstream.headers());

        // (12) Response body stream with optional size cap. Cap
        // enforcement is mid-stream so an upstream that advertises
        // `Content-Length: 50 KiB` but then streams 50 MiB is cut off
        // (same contract as the `http.*` step). Permit is held for
        // the duration of the stream — moving it into the stream's
        // state ties the Semaphore slot to end-of-body, which is the
        // correct in-flight semantic.
        let response_cap = self.response_size_limit;
        let stream = capped_response_stream(upstream.bytes_stream(), response_cap, permit);
        let body_out = Body::from_stream(stream);

        let mut builder = Response::builder().status(status);
        if let Some(hdrs) = builder.headers_mut() {
            for (name, value) in response_headers.iter() {
                hdrs.insert(name, value.clone());
            }
        }
        builder.body(body_out).unwrap_or_else(|e| {
            tracing::error!(
                error = %e,
                "proxy: failed to build response — this should be unreachable"
            );
            (StatusCode::BAD_GATEWAY, "proxy response build failed").into_response()
        })
    }
}

/// Build the forwarded request header map:
///   - Starts empty if `preserve_headers = false`, else copies all
///     client headers.
///   - Hop-by-hop list is always stripped (even when
///     preserve_headers = true).
///   - Any header named in the inbound `Connection:` list is also
///     stripped (RFC 7230 §6.1 dynamic hop-by-hop).
///   - `traceparent` is set to the request-scoped value from
///     `DslRouter::handle_request` — forwarded end-to-end for
///     observability.
fn build_forward_request_headers(
    inbound: &HeaderMap,
    preserve_headers: bool,
    traceparent: &str,
) -> HeaderMap {
    let mut out = HeaderMap::new();

    // Dynamic hop-by-hop list from the client's `Connection:` header.
    let mut dynamic_hop: Vec<String> = Vec::new();
    if let Some(conn) = inbound.get("connection").and_then(|v| v.to_str().ok()) {
        for name in conn.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            dynamic_hop.push(name.to_ascii_lowercase());
        }
    }

    if preserve_headers {
        for (name, value) in inbound.iter() {
            let n = name.as_str().to_ascii_lowercase();
            if HOP_BY_HOP_REQUEST.iter().any(|h| *h == n) {
                continue;
            }
            if dynamic_hop.iter().any(|h| h == &n) {
                continue;
            }
            out.insert(name.clone(), value.clone());
        }
    } else {
        // Minimal forwarding set: only Content-Type + Content-Length
        // (reqwest will recompute Content-Length when streaming so
        // this is primarily preserving the type).
        if let Some(ct) = inbound.get("content-type") {
            out.insert(HeaderName::from_static("content-type"), ct.clone());
        }
        if let Some(cl) = inbound.get("content-length") {
            out.insert(HeaderName::from_static("content-length"), cl.clone());
        }
    }

    if let Ok(hv) = HeaderValue::try_from(traceparent) {
        out.insert(HeaderName::from_static("traceparent"), hv);
    }

    out
}

/// Response-leg hop-by-hop strip. Preserves everything else verbatim.
fn strip_response_hop_by_hop(inbound: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    // Dynamic hop-by-hop from upstream's own Connection: header, same
    // RFC §6.1 rule as request leg.
    let mut dynamic_hop: Vec<String> = Vec::new();
    if let Some(conn) = inbound.get("connection").and_then(|v| v.to_str().ok()) {
        for name in conn.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            dynamic_hop.push(name.to_ascii_lowercase());
        }
    }
    for (name, value) in inbound.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP_RESPONSE.iter().any(|h| *h == n) {
            continue;
        }
        if dynamic_hop.iter().any(|h| h == &n) {
            continue;
        }
        if let (Ok(hname), Ok(hval)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out.insert(hname, hval);
        }
    }
    out
}

/// Append the inbound request's query string to the declared upstream.
/// If upstream already contains `?`, inbound params are joined with
/// `&`; otherwise `?` is used.
fn merge_query(upstream: &str, inbound_query: Option<&str>) -> std::result::Result<String, String> {
    let q = match inbound_query {
        Some(q) if !q.is_empty() => q,
        _ => return Ok(upstream.to_string()),
    };
    let sep = if upstream.contains('?') { '&' } else { '?' };
    Ok(format!("{}{}{}", upstream, sep, q))
}

/// Wrap the inbound body stream with:
///   1. a running byte count that errors on `total > max_body_bytes`,
///   2. an idle-frame timeout that errors if no frame arrives within
///      `idle_timeout` (slowloris mitigation).
///
/// Returns a Stream<Item = Result<Bytes, io::Error>> suitable for
/// `reqwest::Body::wrap_stream`.
fn metered_request_body(
    inner: axum::body::BodyDataStream,
    max_body_bytes: u64,
    idle_timeout: Option<Duration>,
) -> impl futures::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + 'static {
    use std::io;
    let counter = Arc::new(AtomicU64::new(0));
    futures::stream::unfold(
        (inner, counter, max_body_bytes, idle_timeout, false),
        |(mut stream, counter, cap, idle, errored)| async move {
            if errored {
                return None;
            }
            let next_frame = stream.next();
            let frame = match idle {
                Some(t) => match tokio::time::timeout(t, next_frame).await {
                    Ok(f) => f,
                    Err(_) => {
                        return Some((
                            Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "proxy inbound idle timeout",
                            )),
                            (stream, counter, cap, idle, true),
                        ));
                    }
                },
                None => next_frame.await,
            };
            match frame {
                None => None,
                Some(Ok(bytes)) => {
                    let n = bytes.len() as u64;
                    let total = counter.fetch_add(n, Ordering::Relaxed) + n;
                    if total > cap {
                        Some((
                            Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "proxy inbound body exceeded cap {} bytes (seen {})",
                                    cap, total
                                ),
                            )),
                            (stream, counter, cap, idle, true),
                        ))
                    } else {
                        Some((Ok(bytes), (stream, counter, cap, idle, false)))
                    }
                }
                Some(Err(e)) => Some((
                    Err(io::Error::other(format!("proxy inbound body error: {}", e))),
                    (stream, counter, cap, idle, true),
                )),
            }
        },
    )
}

/// Wrap the upstream response body stream with a running byte count
/// that errors on `total > response_size_limit`. Also carries the
/// Semaphore permit so it is only released when the body is fully
/// consumed (or dropped) — this is the correct "in-flight" semantic
/// for backpressure (next 503 at the right queue depth).
fn capped_response_stream(
    inner: impl futures::Stream<Item = std::result::Result<Bytes, reqwest::Error>>
        + Send
        + Unpin
        + 'static,
    cap: Option<usize>,
    permit: Option<OwnedSemaphorePermit>,
) -> impl futures::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + 'static {
    use std::io;
    let counter = Arc::new(AtomicU64::new(0));
    futures::stream::unfold(
        (inner, counter, cap, permit, false),
        |(mut stream, counter, cap, permit, errored)| async move {
            if errored {
                return None;
            }
            match stream.next().await {
                None => {
                    drop(permit);
                    None
                }
                Some(Ok(bytes)) => {
                    let n = bytes.len() as u64;
                    let total = counter.fetch_add(n, Ordering::Relaxed) + n;
                    if let Some(limit) = cap {
                        if total > limit as u64 {
                            drop(permit);
                            return Some((
                                Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!(
                                        "proxy response body exceeded cap {} bytes (seen {})",
                                        limit, total
                                    ),
                                )),
                                (stream, counter, cap, None, true),
                            ));
                        }
                    }
                    Some((Ok(bytes), (stream, counter, cap, permit, false)))
                }
                Some(Err(e)) => {
                    drop(permit);
                    Some((
                        Err(io::Error::other(format!(
                            "proxy response body error: {}",
                            e
                        ))),
                        (stream, counter, cap, None, true),
                    ))
                }
            }
        },
    )
}

// --------------------------------------------------------------------
// Error-response builders. All proxy-owned errors emit a structured
// JSON body with a stable `error` field + diagnostic context, matching
// the existing shapes used by the body-cap / multipart-cap / timeout
// layers. Clients hard-coded to "4xx = bad request, 5xx = our fault"
// can rely on the status code; richer clients get the detail they need
// via the body.
// --------------------------------------------------------------------

fn body_too_large_preflight(declared: u64, cap: u64) -> Response<Body> {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(json!({
            "error": "proxy_body_too_large",
            "declared": declared,
            "cap": cap,
            "message": format!(
                "declared Content-Length {} exceeds proxy route cap {}",
                declared, cap
            ),
        })),
    )
        .into_response()
}

fn unsupported_encoding(seen: &str, allowed: &[String]) -> Response<Body> {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        axum::Json(json!({
            "error": "proxy_unsupported_encoding",
            "seen": seen,
            "allowed": allowed,
            "message": format!(
                "Content-Encoding '{}' not permitted; allowed: {}",
                seen,
                allowed.join(", ")
            ),
        })),
    )
        .into_response()
}

fn proxy_upstream_rejected(reason: &str) -> Response<Body> {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({
            "error": "proxy_upstream_rejected",
            "message": reason,
        })),
    )
        .into_response()
}

fn capacity_exceeded(cap: u32) -> Response<Body> {
    let mut resp = (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({
            "error": "proxy_capacity_exceeded",
            "cap": cap,
            "message": "proxy route in-flight concurrency cap reached",
        })),
    )
        .into_response();
    if let Ok(hv) = HeaderValue::from_str("1") {
        resp.headers_mut().insert("retry-after", hv);
    }
    resp
}

fn transport_error_response(kind: &str, detail: &str) -> Response<Body> {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({
            "error": "proxy_transport_error",
            "kind": kind,
            "message": detail,
        })),
    )
        .into_response()
}

//! Tiny in-process mock upstream. Used by `mock-http` and
//! `trigger-inject` test modes.
//!
//! We deliberately don't use `mockito` here so the mock server can be
//! spawned from a normal (non-test) binary. It's a thin axum wrapper:
//! POST/GET/PUT/PATCH/DELETE all funnel through one handler that
//! matches on URL substring, records the call, and returns the
//! canned response the test registered.

use crate::testkit::schema::{MockAssertion, MockUpstream};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct CallRecord {
    pub method: String,
    pub url: String,
    pub body: Value,
}

#[derive(Default)]
struct Inner {
    mocks: Vec<MockUpstream>,
    calls: Vec<CallRecord>,
}

/// Handle to a running mock upstream. Cheap to clone.
#[derive(Clone)]
pub struct MockServer {
    inner: Arc<Mutex<Inner>>,
    base_url: String,
    shutdown: Arc<tokio::sync::Notify>,
}

impl MockServer {
    pub async fn spawn() -> anyhow::Result<Self> {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let base_url = format!("http://{}", addr);

        let state = MockServerState {
            inner: inner.clone(),
        };
        let app = Router::new()
            .fallback(any(handle))
            .with_state(state);

        let shutdown_signal = shutdown.clone();
        tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown_signal.notified().await });
            let _ = server.await;
        });

        Ok(Self {
            inner,
            base_url,
            shutdown,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn register(&self, mocks: &[MockUpstream]) {
        let mut inner = self.inner.lock().unwrap();
        inner.mocks.extend(mocks.iter().cloned());
    }

    /// Wipe recorded calls (not mocks). Useful between scenarios in
    /// the same file.
    pub fn reset_calls(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.clear();
    }

    /// Wipe everything.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.mocks.clear();
        inner.calls.clear();
    }

    pub fn calls(&self) -> Vec<CallRecord> {
        self.inner.lock().unwrap().calls.clone()
    }

    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    /// Evaluate one assertion. Returns `Ok(())` if it passes, `Err(msg)`
    /// otherwise.
    pub fn assert(&self, a: &MockAssertion) -> Result<(), String> {
        let calls = self.calls();
        let matching: Vec<&CallRecord> = calls
            .iter()
            .filter(|c| c.url.contains(&a.url_matches))
            .collect();

        if matching.len() != a.count {
            return Err(format!(
                "mock assertion failed: expected {} call(s) matching '{}', got {} (all calls: {:?})",
                a.count,
                a.url_matches,
                matching.len(),
                calls.iter().map(|c| (&c.method, &c.url)).collect::<Vec<_>>()
            ));
        }

        if let Some(expected_body) = &a.body_matches {
            for (i, c) in matching.iter().enumerate() {
                if !crate::testkit::matcher::subset_matches(expected_body, &c.body) {
                    return Err(format!(
                        "mock call #{} body does not match expected subset\n  expected: {}\n  actual:   {}",
                        i, expected_body, c.body
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
struct MockServerState {
    inner: Arc<Mutex<Inner>>,
}

async fn handle(
    State(state): State<MockServerState>,
    method: Method,
    uri: Uri,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    let url_str = uri.to_string();
    let body_json: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::String(
            String::from_utf8_lossy(&body).to_string(),
        ))
    };

    let response = {
        let mut inner = state.inner.lock().unwrap();
        inner.calls.push(CallRecord {
            method: method.to_string(),
            url: url_str.clone(),
            body: body_json,
        });

        // First mock whose URL substring matches AND method matches wins.
        inner
            .mocks
            .iter()
            .find(|m| {
                url_str.contains(&m.url_matches)
                    && m.method.eq_ignore_ascii_case(method.as_str())
            })
            .cloned()
    };

    let mock = match response {
        Some(m) => m,
        None => {
            // No mock registered — 599 so the DSL fails loudly.
            return (
                StatusCode::from_u16(599).unwrap(),
                Json(serde_json::json!({
                    "error": "no mock registered",
                    "method": method.to_string(),
                    "url": url_str,
                })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(mock.status).unwrap_or(StatusCode::OK);
    let mut resp = if let Some(body) = mock.body {
        (status, Json(body)).into_response()
    } else {
        status.into_response()
    };
    for (k, v) in &mock.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::try_from(v.as_str()),
        ) {
            resp.headers_mut().insert(name, value);
        }
    }
    resp
}

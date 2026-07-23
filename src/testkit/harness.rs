//! Runtime harness used by `dsl-test`. One `Harness` owns a fully
//! built router + engine + trigger dispatcher for one test file (so
//! per-file constant overrides can produce a fresh DSL tree without
//! bleeding into other files).
//!
//! `execute_http` routes through the real axum stack via
//! `tower::ServiceExt::oneshot` so framework-layer behaviour
//! (CSRF, traceparent, method allow-list) is
//! exercised alongside the DSL itself. In-memory, no port bind.

use crate::config::AppConfig;
use crate::dsl::loader::DslLoader;
use crate::http_client::HttpClient;
use crate::router::DslRouter;
use crate::state::StateStore;
use crate::steps::engine::StepEngine;
use crate::triggers::TriggerDispatcher;
use crate::ws::WsRegistry;
use crate::Result;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

pub struct Harness {
    pub app: axum::Router,
    pub state: StateStore,
    pub trigger: Arc<TriggerDispatcher>,
    pub config: AppConfig,
}

pub struct HarnessResponse {
    pub status: u16,
    pub body: Value,
    pub headers: HashMap<String, String>,
}

impl Harness {
    /// Build a harness by loading the DSL tree from `dsl_root` with
    /// the given constants applied.
    pub fn build(dsl_root: &Path, constants: HashMap<String, String>) -> Result<Self> {
        let config = AppConfig {
            config_path: dsl_root.to_path_buf(),
            ..AppConfig::default()
        };
        Self::build_with_config(config, constants)
    }

    /// Build a harness with a caller-supplied `AppConfig`. Callers must
    /// still set `config.config_path` themselves; nothing here overrides
    /// it. Used by the mock-http and trigger-inject test modes to relax
    /// N4's `block_private_networks` default so the DSL under test can
    /// reach the mock server bound on 127.0.0.1 without disabling SSRF
    /// hardening in the production code path.
    pub fn build_with_config(
        config: AppConfig,
        constants: HashMap<String, String>,
    ) -> Result<Self> {
        let loader = DslLoader::new(config.clone(), constants);
        let loaded = loader.load_everything()?;

        let state = StateStore::new();
        let ws_registry = WsRegistry::new();
        let http_client = HttpClient::new(&config);
        let shared_http_dsls = Arc::new(loaded.http);
        let mut engine = StepEngine::new(http_client)
            .with_ws_registry(ws_registry.clone())
            .with_dsls(shared_http_dsls.clone());
        if let Some(n) = config.max_step_recursions {
            engine = engine.with_max_iterations(n);
        }

        let trigger = Arc::new(TriggerDispatcher::new(
            loaded.triggers,
            state.clone(),
            engine.clone(),
        ));

        let router = DslRouter::from_arc(
            shared_http_dsls,
            loaded.guards,
            config.clone(),
            state.clone(),
            ws_registry,
            engine,
        );
        let app = router.build_axum_router();

        Ok(Self {
            app,
            state,
            trigger,
            config,
        })
    }

    /// Fire one HTTP request through the built router. Uses
    /// `tower::ServiceExt::oneshot` so full middleware runs (CSRF,
    /// traceparent, method allow-list) — no TCP socket.
    pub async fn execute_http(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &HashMap<String, Value>,
        headers: &HashMap<String, String>,
    ) -> anyhow::Result<HarnessResponse> {
        let m = Method::from_bytes(method.to_uppercase().as_bytes())?;

        let mut uri = path.to_string();
        if !query.is_empty() {
            let qs: Vec<String> = query
                .iter()
                .map(|(k, v)| {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("{}={}", url_encode(k), url_encode(&s))
                })
                .collect();
            uri.push('?');
            uri.push_str(&qs.join("&"));
        }

        let mut req_builder = Request::builder().method(m).uri(uri);
        for (k, v) in headers {
            req_builder = req_builder.header(k, v);
        }

        let body_bytes = match body {
            Some(b) => serde_json::to_vec(b)?,
            None => Vec::new(),
        };
        let req = req_builder.body(Body::from(body_bytes))?;

        let resp = self.app.clone().oneshot(req).await?;
        let status = resp.status().as_u16();
        let resp_headers: HashMap<String, String> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // 16 MiB parity with the live router.
        let body_bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await?;
        let body_value: Value = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body_bytes).to_string()))
        };

        // NOT_FOUND / server-injected errors surface here just like any
        // other response; callers assert on `status`.
        let _ = StatusCode::from_u16(status);

        Ok(HarnessResponse {
            status,
            body: body_value,
            headers: resp_headers,
        })
    }
}

fn url_encode(s: &str) -> String {
    // Extremely minimal — sufficient for tests where params are simple
    // ASCII. Punts on non-ASCII by percent-encoding via url::form_urlencoded.
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

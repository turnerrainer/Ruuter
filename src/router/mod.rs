use crate::config::AppConfig;
use crate::context::ExecutionContext;
use crate::dsl::loader::GuardDsls;
use crate::dsl::Dsl;
use crate::http_client::{HttpResponse, SelfCallHandler};
use crate::idempotency::IdempotencyStore;
use crate::state::StateStore;
use crate::steps::engine::{DslExecutionResult, StepEngine};
use crate::ws::{random_client_id, Outbound, WsRegistry};
use crate::{Result, RuuterError};
use async_trait::async_trait;
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        FromRequestParts, Request, State,
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use rand::Rng;
use tower_http::cors::{AllowOrigin, CorsLayer};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, warn};

pub struct DslRouter {
    dsls: Arc<crate::dsl::loader::HttpDsls>,
    guards: GuardDsls,
    config: AppConfig,
    engine: StepEngine,
    state: StateStore,
    ws_registry: WsRegistry,
    idempotency: IdempotencyStore,
    openapi_spec: Arc<Value>,
}

impl DslRouter {
    pub fn new(
        dsls: HashMap<String, HashMap<String, HashMap<String, Dsl>>>,
        guards: GuardDsls,
        config: AppConfig,
        state: StateStore,
        ws_registry: WsRegistry,
        engine: StepEngine,
    ) -> Self {
        Self::from_arc(Arc::new(dsls), guards, config, state, ws_registry, engine)
    }

    /// Construct from a shared `Arc<HttpDsls>`. Preferred when the same
    /// tree is also handed to the StepEngine (`with_dsls`) so that
    /// template steps and HTTP routing operate on the same in-memory
    /// view without duplicating the tree.
    pub fn from_arc(
        dsls: Arc<crate::dsl::loader::HttpDsls>,
        guards: GuardDsls,
        config: AppConfig,
        state: StateStore,
        ws_registry: WsRegistry,
        engine: StepEngine,
    ) -> Self {
        let idempotency = IdempotencyStore::new(
            std::time::Duration::from_secs(config.idempotency.ttl_seconds),
        );
        let openapi_spec = Arc::new(
            crate::openapi::build_spec_from_http(&dsls, env!("CARGO_PKG_VERSION"))
        );
        Self {
            dsls,
            guards,
            engine,
            state,
            config,
            ws_registry,
            idempotency,
            openapi_spec,
        }
    }

    /// Java-parity DSL lookup: try the exact key, then progressively
    /// strip trailing path segments. Each stripped segment is
    /// prepended to `path_params` so the final vector matches URL
    /// order. Returns `(matched_dsl, matched_key, path_params)` on
    /// success; `None` if no shortened key resolves. The matched key
    /// is what guards prefix-match against.
    fn resolve_dsl_with_path_params(
        &self,
        project: &str,
        method: &str,
        path: &str,
    ) -> Option<(&Dsl, String, Vec<String>)> {
        let by_method = self.dsls.get(project)?.get(method)?;
        let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut stripped: Vec<String> = Vec::new();

        loop {
            if segments.is_empty() {
                return None;
            }
            let candidate = format!("{}/{}", method, segments.join("/"));
            if let Some(dsl) = by_method.get(&candidate) {
                return Some((dsl, candidate, stripped));
            }
            // Strip the trailing segment; prepend to keep URL order.
            if let Some(last) = segments.pop() {
                stripped.insert(0, last.to_string());
            } else {
                return None;
            }
        }
    }

    /// Return the list of guard DSLs that protect `dsl_key`, ordered
    /// outermost-first. A guard at key `<METHOD>/path/<stem>` applies
    /// to every DSL whose key starts with `<METHOD>/path/<stem>/`.
    ///
    /// Task 020 — if ANY matching guard has
    /// `declaration.override_ancestors: true`, ancestor guards are
    /// skipped and only the closest-match (longest key) override guard
    /// runs. Non-override guards stack normally when no override is
    /// present anywhere on the path.
    fn applicable_guards(&self, project: &str, dsl_key: &str) -> Vec<Dsl> {
        let project_guards = match self.guards.get(project) {
            Some(g) => g,
            None => return Vec::new(),
        };
        let mut matches: Vec<(usize, Dsl)> = Vec::new();
        for (guard_key, guard_dsl) in project_guards {
            let prefix_with_slash = format!("{}/", guard_key);
            if dsl_key.starts_with(&prefix_with_slash) {
                matches.push((guard_key.len(), guard_dsl.clone()));
            }
        }
        // Outer-first: shorter key (= broader scope) runs before nested.
        matches.sort_by_key(|(len, _)| *len);

        // Override handling: if any matching guard declares
        // `override_ancestors: true`, keep ONLY the longest-key override
        // guard. Multiple overrides → most-specific wins.
        let has_override = matches
            .iter()
            .any(|(_, d)| is_override_guard(d));
        if has_override {
            let longest_override = matches
                .iter()
                .filter(|(_, d)| is_override_guard(d))
                .max_by_key(|(len, _)| *len)
                .map(|(_, d)| d.clone());
            return longest_override.into_iter().collect();
        }

        matches.into_iter().map(|(_, d)| d).collect()
    }

    pub fn build_axum_router(self) -> Router {
        Arc::new(self).build_axum_router_from_arc()
    }

    /// Task 044 wiring: build the axum router from a pre-existing
    /// `Arc<Self>` so `main.rs` can share the same handle with
    /// `HttpClient` (for self-call short-circuit) BEFORE consuming
    /// the router into axum's state. Without this, HttpClient would
    /// have no live reference to the router by the time it needs to
    /// dispatch a self-call.
    pub fn build_axum_router_from_arc(self: Arc<Self>) -> Router {
        let cors = build_cors_layer(&self.config.cors);
        let state = self;

        let mut router = Router::new()
            .route("/health", get(health_check))
            .route("/_/openapi.json", get(openapi_handler))
            .fallback(any(handle_request))
            .with_state(state);
        if let Some(layer) = cors {
            router = router.layer(layer);
        }
        router
    }

    pub async fn execute_dsl(
        &self,
        project: &str,
        method: &str,
        path: &str,
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        origin: String,
    ) -> Result<DslExecutionResult> {
        // Java-parity path-param lookup (task 018). Try the exact DSL
        // key first; on miss, strip trailing path segments one at a
        // time and prepend each stripped value to `pathParams`. This
        // lets a single `GET/things.yml` serve `/things`,
        // `/things/{id}`, `/things/{id}/legs`, …
        let uppercase_method = method.to_uppercase();
        let (dsl, matched_key, path_params) = self
            .resolve_dsl_with_path_params(project, &uppercase_method, path)
            .ok_or_else(|| {
                RuuterError::FileNotFound(format!("DSL not found: {}/{}", uppercase_method, path))
            })?;
        let dsl = dsl.clone();

        // Expose the stripped segments to the DSL under
        // `incoming.params.pathParams`. Empty array when the exact key
        // matched — DSLs can check `pathParams.length` to branch.
        let mut query = query;
        query.insert(
            "pathParams".to_string(),
            Value::Array(path_params.into_iter().map(Value::String).collect()),
        );

        // Adopt or generate traceparent. Either way ExecutionContext holds
        // it and the http step forwards it to downstream calls.
        let traceparent = headers
            .get("traceparent")
            .cloned()
            .unwrap_or_else(generate_traceparent);
        let mut new_headers = headers;
        new_headers.insert("traceparent".to_string(), traceparent.clone());

        let context = ExecutionContext::with_state(
            body, query, new_headers, origin,
            project.to_string(),
            self.state.clone(),
        )
        .with_traceparent(traceparent);

        // Run any guards that protect this route, outermost first.
        // A guard returning a >= 400 status short-circuits — its
        // response (status, body) becomes the response, the main DSL
        // does not run. The same `ExecutionContext` flows through, so
        // a guard can `assign` variables (e.g. a parsed token) for
        // the main DSL to consume.
        for guard in self.applicable_guards(project, &matched_key) {
            let guard_result = self.engine.run(&guard, &context).await?;
            if guard_result.status >= 400 {
                return Ok(guard_result);
            }
        }

        self.engine.run(&dsl, &context).await
    }
}

/// Task 044 — DslRouter implements SelfCallHandler so HttpClient can
/// short-circuit `http.<verb>` calls that target Ruuter's own listener
/// back into the router without the round trip through reqwest and
/// the framework's own accept loop.
#[async_trait]
impl SelfCallHandler for DslRouter {
    async fn execute_by_url(
        &self,
        method: &str,
        url_path: &str,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        body: HashMap<String, Value>,
    ) -> Result<HttpResponse> {
        // Split off the leading project segment. `/samples/basic/hello`
        // → project="samples", path="basic/hello". Matches how
        // `handle_request` parses inbound URIs before calling execute_dsl.
        let trimmed = url_path.trim_start_matches('/');
        let (project, path) = match trimmed.find('/') {
            Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
            None => (trimmed, ""),
        };
        let result = self
            .execute_dsl(
                project,
                method,
                path,
                body,
                query,
                headers,
                "self-call".to_string(),
            )
            .await?;
        Ok(HttpResponse {
            status: result.status,
            body: result.value,
            headers: result.headers,
        })
    }
}

async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "ruuter-on-rust",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Serve the OpenAPI 3.1 spec generated from the loaded DSL tree at
/// boot. Cheap — served straight from the `Arc<Value>` cache; no
/// per-request work.
async fn openapi_handler(State(router): State<Arc<DslRouter>>) -> impl IntoResponse {
    Json((*router.openapi_spec).clone())
}

async fn handle_request(
    State(router): State<Arc<DslRouter>>,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();

    // Detect WebSocket upgrade up front so we never try to read a body
    // off a hijacked connection. Same path namespace as HTTP — the
    // lookup just resolves to `WS/<path>.yml` instead of `<METHOD>/...`.
    let is_ws_upgrade = headers
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    if is_ws_upgrade && method == Method::GET {
        let (mut parts, _body) = request.into_parts();
        return match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => router.handle_ws_upgrade(upgrade, uri, headers).await,
            Err(rej) => rej.into_response(),
        };
    }

    // Method allow-list — operator can lock the surface to a subset. Default
    // is all standard REST verbs (see IncomingRequestsConfig default).
    let allowed = &router.config.incoming_requests.allowed_method_types;
    if !allowed.iter().any(|m| m.eq_ignore_ascii_case(method.as_str())) {
        return (StatusCode::METHOD_NOT_ALLOWED, Json(json!({"error": "Method Not Allowed"}))).into_response();
    }

    // CSRF: on state-changing methods, require Origin (or fall back to
    // Referer) matching the configured allow-list. Bypassed when
    // csrf.allowed_origins is empty — same-origin admin surfaces behind
    // reverse proxies can rely on SameSite=Strict cookies alone.
    if !router.config.csrf.allowed_origins.is_empty()
        && router
            .config
            .csrf
            .enforce_on_methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method.as_str()))
    {
        if !origin_allowed(&headers, &router.config.csrf.allowed_origins) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "CSRF: origin not allowed"})),
            )
                .into_response();
        }
    }

    // Optimistic-concurrency: reject state-changing calls without If-Match
    // when the operator has opted in. Actual token validation is a Resql
    // concern executed by the DSL body.
    if router.config.optimistic_concurrency.require_if_match
        && router
            .config
            .optimistic_concurrency
            .enforce_on_methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method.as_str()))
        && !headers.contains_key(axum::http::header::IF_MATCH)
    {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({"error": "If-Match header is required for this method"})),
        )
            .into_response();
    }

    let path_parts: Vec<&str> = uri.path().split('/').filter(|s| !s.is_empty()).collect();

    if path_parts.is_empty() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let project = path_parts[0].to_string();
    let endpoint_path = path_parts[1..].join("/");

    let query_params: HashMap<String, String> = uri
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();

    let query: HashMap<String, Value> = query_params
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    let headers_map: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    // Read body bytes (up to 16 MiB) then parse as JSON object.
    let body_bytes = match axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("body read error: {}", e) })),
            )
                .into_response();
        }
    };
    // Only try to parse a JSON body when the client declared one. This lets
    // form-encoded / multipart / plain-text callers reach the DSL with an
    // empty `incoming.body` map, while a client that says
    // `Content-Type: application/json` and sends malformed JSON gets a 400
    // instead of silently losing its body.
    let content_type = headers_map
        .get("content-type")
        .map(String::as_str)
        .unwrap_or("");
    let looks_json = content_type
        .split(';')
        .next()
        .map(str::trim)
        .map(|mime| mime.eq_ignore_ascii_case("application/json") || mime.ends_with("+json"))
        .unwrap_or(false);

    let body_map: HashMap<String, Value> = if body_bytes.is_empty() {
        HashMap::new()
    } else if looks_json {
        match serde_json::from_slice::<Value>(&body_bytes) {
            Ok(Value::Object(map)) => map.into_iter().collect(),
            Ok(other) => {
                // Non-object JSON — wrap under `value` so a DSL can still
                // reach it via `incoming.body.value`, matching the WS shape.
                let mut wrapper = HashMap::new();
                wrapper.insert("value".to_string(), other);
                wrapper
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("invalid JSON body: {}", e) })),
                )
                    .into_response();
            }
        }
    } else {
        HashMap::new()
    };

    let origin = headers_map
        .get("x-forwarded-for")
        .or_else(|| headers_map.get("x-real-ip"))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    // Idempotency-Key dedup — if a matching request has been cached,
    // replay its response verbatim rather than re-running the DSL.
    let idem_key_header = headers_map.get("idempotency-key").cloned();
    let idem_active = router.config.idempotency.enabled
        && idem_key_header.is_some()
        && router
            .config
            .idempotency
            .methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method.as_str()));

    let dedup_key = idem_key_header.as_ref().map(|k| {
        IdempotencyStore::dedup_key(k, method.as_str(), &project, &endpoint_path)
    });

    if idem_active {
        if let Some(key) = dedup_key {
            if let Some(cached) = router.idempotency.get(&key) {
                let status = StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK);
                let mut resp =
                    (status, Json(cached.body.unwrap_or(json!({})))).into_response();
                for (k, v) in cached.headers {
                    if let (Ok(name), Ok(value)) = (
                        HeaderName::try_from(k.as_str()),
                        HeaderValue::try_from(v.as_str()),
                    ) {
                        resp.headers_mut().insert(name, value);
                    }
                }
                if let Ok(v) = HeaderValue::try_from("true") {
                    resp.headers_mut().insert(HeaderName::from_static("idempotency-replayed"), v);
                }
                if let Some(k) = idem_key_header.as_ref() {
                    if let Ok(v) = HeaderValue::try_from(k.as_str()) {
                        resp.headers_mut().insert(HeaderName::from_static("idempotency-key"), v);
                    }
                }
                // A replay still deserves traceparent + X-Trace-Id so
                // ops can correlate across the original + replay in
                // Tempo/Jaeger.
                let replay_tp = headers
                    .get("traceparent")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
                    .unwrap_or_else(generate_traceparent);
                if let Ok(v) = HeaderValue::try_from(replay_tp.as_str()) {
                    resp.headers_mut().insert(HeaderName::from_static("traceparent"), v);
                }
                if let Some(tid) = trace_id_from(&replay_tp) {
                    if let Ok(v) = HeaderValue::try_from(tid) {
                        resp.headers_mut().insert(HeaderName::from_static("x-trace-id"), v);
                    }
                }
                apply_default_response_headers(&mut resp, &router.config.response_default_headers);
                return resp;
            }
        }
    }

    let dsl_outcome = router
        .execute_dsl(
            &project,
            method.as_str(),
            &endpoint_path,
            body_map,
            query,
            headers_map,
            origin,
        )
        .await;

    // Snapshot the result so we can both build a response AND persist it
    // in the idempotency store without re-executing.
    let (status_code, body_value, extra_headers, exec_ok) = match &dsl_outcome {
        Ok(result) => (
            StatusCode::from_u16(result.status).unwrap_or(StatusCode::OK),
            result.value.clone().unwrap_or(json!({})),
            result.headers.clone(),
            true,
        ),
        Err(RuuterError::FileNotFound(_)) => (
            StatusCode::NOT_FOUND,
            json!({"error": "Not Found"}),
            HashMap::new(),
            false,
        ),
        Err(e) => {
            error!("Error executing DSL: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": e.to_string() }),
                HashMap::new(),
                false,
            )
        }
    };

    if idem_active && exec_ok {
        if let Some(key) = dedup_key {
            router
                .idempotency
                .insert(key, status_code.as_u16(), Some(body_value.clone()), extra_headers.clone());
        }
    }

    let mut response = match &dsl_outcome {
        Ok(_) => {
            let mut resp = (status_code, Json(body_value)).into_response();
            for (k, v) in extra_headers {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::try_from(k.as_str()),
                    HeaderValue::try_from(v.as_str()),
                ) {
                    resp.headers_mut().insert(name, value);
                }
            }
            if let Some(k) = idem_key_header.as_ref() {
                if idem_active {
                    if let Ok(v) = HeaderValue::try_from(k.as_str()) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("idempotency-key"), v);
                    }
                }
            }
            resp
        }
        Err(_) => (status_code, Json(body_value)).into_response(),
    };

    // Echo traceparent + X-Trace-Id so ops can correlate a client-side
    // request id with server-side traces. If the caller sent a traceparent
    // we adopt it verbatim; otherwise we generated one at execute_dsl
    // time — but that generation is inside execute_dsl and we don't have
    // the string back here, so we compute it again from what the caller
    // sent or generate a matching pair for the response only.
    let response_traceparent = headers
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(generate_traceparent);
    if let Ok(v) = HeaderValue::try_from(response_traceparent.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("traceparent"), v);
    }
    if let Some(tid) = trace_id_from(&response_traceparent) {
        if let Ok(v) = HeaderValue::try_from(tid) {
            response
                .headers_mut()
                .insert(HeaderName::from_static("x-trace-id"), v);
        }
    }

    apply_default_response_headers(&mut response, &router.config.response_default_headers);
    response
}

fn apply_default_response_headers(response: &mut Response, defaults: &HashMap<String, String>) {
    if defaults.is_empty() {
        return;
    }
    let headers = response.headers_mut();
    for (k, v) in defaults {
        let name = match HeaderName::try_from(k.as_str()) {
            Ok(n) => n,
            Err(_) => continue,
        };
        // Don't clobber a header a DSL explicitly set — defaults are, well,
        // defaults.
        if headers.contains_key(&name) {
            continue;
        }
        if let Ok(value) = HeaderValue::try_from(v.as_str()) {
            headers.insert(name, value);
        }
    }
}

impl DslRouter {
    /// Resolve a WS DSL by `(project, path)` and, on hit, accept the
    /// upgrade and spawn the per-connection task.
    async fn handle_ws_upgrade(
        self: Arc<Self>,
        upgrade: WebSocketUpgrade,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        let path_parts: Vec<&str> = uri.path().split('/').filter(|s| !s.is_empty()).collect();
        if path_parts.len() < 2 {
            return (StatusCode::NOT_FOUND, "Not found").into_response();
        }
        let project = path_parts[0].to_string();
        let endpoint_path = path_parts[1..].join("/");
        let dsl_key = format!("WS/{}", endpoint_path);

        let dsl = match self
            .dsls
            .get(&project)
            .and_then(|methods| methods.get("WS"))
            .and_then(|dsls| dsls.get(&dsl_key))
        {
            Some(d) => d.clone(),
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "Not Found"})))
                    .into_response();
            }
        };

        // Snapshot inbound headers + query so a guard or the WS DSL
        // can read them on connect (e.g. token check). They do NOT
        // change per frame — each frame run sees the same handshake
        // headers in `incoming.headers`.
        let query_params: HashMap<String, String> = uri
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        let headers_map: HashMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let router = self.clone();
        upgrade.on_upgrade(move |socket| async move {
            router
                .run_ws_connection(socket, project, dsl, query_params, headers_map)
                .await;
        })
    }

    async fn run_ws_connection(
        self: Arc<Self>,
        socket: WebSocket,
        project: String,
        dsl: Dsl,
        query_params: HashMap<String, String>,
        headers_map: HashMap<String, String>,
    ) {
        let connection_id = random_client_id();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outbound>();
        self.ws_registry.register(connection_id.clone(), out_tx);

        let (mut sink, mut stream) = socket.split();

        // Writer task — drains the registry channel and writes to the
        // socket. Kept separate so the reader can dispatch frames
        // through the engine without blocking on socket back-pressure.
        let writer = tokio::spawn(async move {
            while let Some(Outbound::Json(value)) = out_rx.recv().await {
                let text = match serde_json::to_string(&value) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, "ws server writer: bad JSON");
                        continue;
                    }
                };
                if sink.send(WsMessage::Text(text)).await.is_err() {
                    break;
                }
            }
        });

        let query: HashMap<String, Value> = query_params
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();

        while let Some(frame) = stream.next().await {
            let frame = match frame {
                Ok(f) => f,
                Err(e) => {
                    warn!(connection_id, error = %e, "ws stream error");
                    break;
                }
            };
            match frame {
                WsMessage::Text(text) => {
                    let payload = parse_payload(&text);
                    self.dispatch_ws_frame(
                        &project,
                        &connection_id,
                        payload,
                        &dsl,
                        &query,
                        &headers_map,
                    )
                    .await;
                }
                WsMessage::Binary(bytes) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        let payload = parse_payload(text);
                        self.dispatch_ws_frame(
                            &project,
                            &connection_id,
                            payload,
                            &dsl,
                            &query,
                            &headers_map,
                        )
                        .await;
                    }
                }
                WsMessage::Close(_) => break,
                _ => {}
            }
        }

        self.ws_registry.unregister(&connection_id);
        writer.abort();
    }

    async fn dispatch_ws_frame(
        &self,
        project: &str,
        connection_id: &str,
        payload: Value,
        dsl: &Dsl,
        query: &HashMap<String, Value>,
        headers: &HashMap<String, String>,
    ) {
        let body = match payload {
            Value::Object(map) => map.into_iter().collect::<HashMap<_, _>>(),
            other => {
                let mut wrapper = HashMap::new();
                wrapper.insert("value".to_string(), other);
                wrapper
            }
        };

        let context = ExecutionContext::with_state(
            body,
            query.clone(),
            headers.clone(),
            "ws".to_string(),
            project.to_string(),
            self.state.clone(),
        )
        .with_connection_id(connection_id.to_string());

        if let Err(e) = self.engine.run(dsl, &context).await {
            warn!(project, connection_id, error = %e, "WS DSL failed");
        }
    }
}

fn is_override_guard(dsl: &Dsl) -> bool {
    dsl.declaration
        .as_ref()
        .and_then(|d| d.override_ancestors)
        .unwrap_or(false)
}

fn parse_payload(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(_) => Value::String(text.to_string()),
    }
}

/// Generate a W3C traceparent when the incoming request didn't carry one.
/// Format: `00-<32 hex trace_id>-<16 hex span_id>-01` (sampled).
fn generate_traceparent() -> String {
    let mut rng = rand::thread_rng();
    let trace_id: u128 = rng.gen();
    let span_id: u64 = rng.gen();
    format!("00-{:032x}-{:016x}-01", trace_id, span_id)
}

fn trace_id_from(traceparent: &str) -> Option<&str> {
    let parts: Vec<&str> = traceparent.splitn(4, '-').collect();
    if parts.len() == 4 && parts[1].len() == 32 {
        Some(parts[1])
    } else {
        None
    }
}

/// Return true if the request's Origin (preferred) or Referer's origin is
/// on the allow-list. A request with neither header is rejected — CORS-
/// preflighted browsers always send Origin; a caller without it is either
/// non-browser (which should authenticate with a Bearer token, not a cookie)
/// or a stripped proxy (which is exactly what the CSRF check catches).
fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let origin_hdr = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(o) = origin_hdr {
        return allowed.iter().any(|a| a == &o);
    }
    let referer_origin = headers
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| url::Url::parse(r).ok())
        .map(|u| format!(
            "{}://{}{}",
            u.scheme(),
            u.host_str().unwrap_or(""),
            u.port().map(|p| format!(":{}", p)).unwrap_or_default(),
        ));
    if let Some(o) = referer_origin {
        return allowed.iter().any(|a| a == &o);
    }
    false
}

/// Build a CORS layer from `cors.allowed_origins`. Returns `None` when no
/// origins are configured — a Ruuter fronted only by a same-origin admin
/// UI does not need CORS at all, so we don't attach a layer that would
/// otherwise send `Access-Control-Allow-Origin: *`.
fn build_cors_layer(cfg: &crate::config::CorsConfig) -> Option<CorsLayer> {
    if cfg.allowed_origins.is_empty() {
        return None;
    }
    let origins: Vec<HeaderValue> = cfg
        .allowed_origins
        .iter()
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        warn!("cors.allowed_origins configured but no entry parsed as a valid HeaderValue");
        return None;
    }
    let mut layer = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers([HeaderName::from_static("content-type"), HeaderName::from_static("authorization"), HeaderName::from_static("idempotency-key"), HeaderName::from_static("if-match"), HeaderName::from_static("traceparent")]);
    if cfg.allow_credentials {
        layer = layer.allow_credentials(true);
    }
    Some(layer)
}

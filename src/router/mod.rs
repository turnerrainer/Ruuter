use crate::config::AppConfig;
use crate::context::ExecutionContext;
use crate::dsl::loader::GuardDsls;
use crate::dsl::Dsl;
use crate::http_client::HttpClient;
use crate::state::StateStore;
use crate::steps::engine::{DslExecutionResult, StepEngine};
use crate::ws::{random_client_id, Outbound, WsRegistry};
use crate::{Result, RuuterError};
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        FromRequestParts, Request, State,
    },
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, warn};

pub struct DslRouter {
    dsls: HashMap<String, HashMap<String, HashMap<String, Dsl>>>,
    guards: GuardDsls,
    #[allow(dead_code)]
    config: AppConfig,
    engine: StepEngine,
    state: StateStore,
    ws_registry: WsRegistry,
}

impl DslRouter {
    pub fn new(
        dsls: HashMap<String, HashMap<String, HashMap<String, Dsl>>>,
        guards: GuardDsls,
        config: AppConfig,
        state: StateStore,
        ws_registry: WsRegistry,
    ) -> Self {
        let http_client = HttpClient::new(config.http_request_timeout);
        let engine = StepEngine::new(http_client).with_ws_registry(ws_registry.clone());
        Self {
            dsls,
            guards,
            engine,
            state,
            config,
            ws_registry,
        }
    }

    /// Return the list of guard DSLs that protect `dsl_key`, ordered
    /// outermost-first. A guard at key `<METHOD>/path/<stem>` applies
    /// to every DSL whose key starts with `<METHOD>/path/<stem>/`.
    fn applicable_guards(&self, project: &str, dsl_key: &str) -> Vec<Dsl> {
        let project_guards = match self.guards.get(project) {
            Some(g) => g,
            None => return Vec::new(),
        };
        // For "GET/protected/data", check ancestors "GET/protected", "GET" — though only those that exist as guard keys actually match.
        let mut matches: Vec<(usize, Dsl)> = Vec::new();
        for (guard_key, guard_dsl) in project_guards {
            let prefix_with_slash = format!("{}/", guard_key);
            if dsl_key.starts_with(&prefix_with_slash) {
                matches.push((guard_key.len(), guard_dsl.clone()));
            }
        }
        // Outer-first: shorter guard key (= broader scope) runs before nested guards.
        matches.sort_by_key(|(len, _)| *len);
        matches.into_iter().map(|(_, d)| d).collect()
    }

    pub fn build_axum_router(self) -> Router {
        let state = Arc::new(self);

        Router::new()
            .route("/health", get(health_check))
            .fallback(any(handle_request))
            .with_state(state)
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
        let dsl_key = format!("{}/{}", method.to_uppercase(), path);

        let dsl = self.dsls
            .get(project)
            .and_then(|methods| methods.get(&method.to_uppercase()))
            .and_then(|dsls| dsls.get(&dsl_key))
            .ok_or_else(|| RuuterError::FileNotFound(format!("DSL not found: {}", dsl_key)))?;

        let context = ExecutionContext::with_state(
            body, query, headers, origin,
            project.to_string(),
            self.state.clone(),
        );

        // Run any guards that protect this route, outermost first.
        // A guard returning a >= 400 status short-circuits — its
        // response (status, body) becomes the response, the main DSL
        // does not run. The same `ExecutionContext` flows through, so
        // a guard can `assign` variables (e.g. a parsed token) for
        // the main DSL to consume.
        for guard in self.applicable_guards(project, &dsl_key) {
            let guard_result = self.engine.run(&guard, &context).await?;
            if guard_result.status >= 400 {
                return Ok(guard_result);
            }
        }

        self.engine.run(dsl, &context).await
    }
}

async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "ruuter-rs",
        "version": env!("CARGO_PKG_VERSION")
    }))
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
    let body_map: HashMap<String, Value> = if body_bytes.is_empty() {
        HashMap::new()
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_default()
    };

    let origin = headers_map
        .get("x-forwarded-for")
        .or_else(|| headers_map.get("x-real-ip"))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    match router
        .execute_dsl(
            &project,
            method.as_str(),
            &endpoint_path,
            body_map,
            query,
            headers_map,
            origin,
        )
        .await
    {
        Ok(result) => {
            let status = StatusCode::from_u16(result.status).unwrap_or(StatusCode::OK);
            let json_response = result.value.unwrap_or(json!({}));
            (status, Json(json_response)).into_response()
        }
        Err(RuuterError::FileNotFound(_)) => {
            // Unmatched route — return a generic 404. Don't echo the
            // attempted DSL key back to the caller; that's information
            // leakage (see #016).
            (StatusCode::NOT_FOUND, Json(json!({"error": "Not Found"}))).into_response()
        }
        Err(e) => {
            error!("Error executing DSL: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
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

fn parse_payload(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(_) => Value::String(text.to_string()),
    }
}

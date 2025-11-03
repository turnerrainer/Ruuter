use crate::config::AppConfig;
use crate::context::ExecutionContext;
use crate::dsl::{loader::DslLoader, Dsl};
use crate::http_client::HttpClient;
use crate::scripting::ScriptEngine;
use crate::steps::*;
use crate::{Result, RuuterError};
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{error, info};

pub struct DslRouter {
    dsls: HashMap<String, HashMap<String, HashMap<String, Dsl>>>,
    config: AppConfig,
    http_client: HttpClient,
}

impl DslRouter {
    pub fn new(dsls: HashMap<String, HashMap<String, HashMap<String, Dsl>>>, config: AppConfig) -> Self {
        let http_client = HttpClient::new(config.http_request_timeout);
        Self {
            dsls,
            config,
            http_client,
        }
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

        let context = ExecutionContext::new(body, query, headers, origin);
        let result = self.execute_steps(dsl, &context).await?;

        Ok(result)
    }

    async fn execute_steps(&self, dsl: &Dsl, context: &ExecutionContext) -> Result<DslExecutionResult> {
        let step_names = dsl.step_names();
        let mut current_step_idx = 0;
        let mut max_iterations = 100;

        while current_step_idx < step_names.len() && max_iterations > 0 {
            max_iterations -= 1;

            let step_name = &step_names[current_step_idx];
            let step = dsl.get_step(step_name)
                .ok_or_else(|| RuuterError::InvalidStep(format!("Step not found: {}", step_name)))?;

            let result = self.execute_single_step(step, context).await?;

            if result.should_return {
                return Ok(DslExecutionResult {
                    value: result.return_value,
                    status: result.return_status.unwrap_or(200),
                    headers: result.return_headers.unwrap_or_default(),
                });
            }

            if let Some(next) = result.next_step {
                if next == "end" {
                    break;
                }
                if let Some(idx) = step_names.iter().position(|n| n == &next) {
                    current_step_idx = idx;
                } else {
                    break;
                }
            } else {
                current_step_idx += 1;
            }
        }

        Ok(DslExecutionResult {
            value: None,
            status: 200,
            headers: HashMap::new(),
        })
    }

    async fn execute_single_step(&self, step: &DslStep, context: &ExecutionContext) -> Result<StepResult> {
        match step {
            DslStep::Assign(s) => {
                let executor = assign::AssignStepExecutor::new(s.clone());
                executor.execute(context).await
            }
            DslStep::Return(s) => {
                let executor = return_step::ReturnStepExecutor::new(s.clone());
                executor.execute(context).await
            }
            DslStep::Http(s) => {
                let executor = http::HttpStepExecutor::new(s.clone(), self.http_client.clone());
                executor.execute(context).await
            }
            DslStep::Switch(s) => {
                let executor = switch::SwitchStepExecutor::new(s.clone());
                executor.execute(context).await
            }
            DslStep::Log(s) => {
                let executor = log::LogStepExecutor::new(s.clone());
                executor.execute(context).await
            }
            DslStep::Template(s) => {
                let executor = template::TemplateStepExecutor::new(s.clone());
                executor.execute(context).await
            }
            DslStep::Declaration(_) => {
                Ok(StepResult::with_next("end".to_string()))
            }
        }
    }
}

impl Clone for HttpClient {
    fn clone(&self) -> Self {
        HttpClient::new(self.default_timeout.as_millis() as u64)
    }
}

#[derive(Debug)]
pub struct DslExecutionResult {
    pub value: Option<Value>,
    pub status: u16,
    pub headers: HashMap<String, String>,
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
    method: Method,
    AxumPath(path): AxumPath<String>,
    Query(query_params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Option<Json<HashMap<String, Value>>>,
) -> Response {
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if path_parts.is_empty() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let project = path_parts[0];
    let endpoint_path = path_parts[1..].join("/");

    let query: HashMap<String, Value> = query_params
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    let headers_map: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body_map = body.map(|b| b.0).unwrap_or_default();

    let origin = headers_map
        .get("x-forwarded-for")
        .or_else(|| headers_map.get("x-real-ip"))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    match router.execute_dsl(
        project,
        method.as_str(),
        &endpoint_path,
        body_map,
        query,
        headers_map,
        origin,
    ).await {
        Ok(result) => {
            let status = StatusCode::from_u16(result.status).unwrap_or(StatusCode::OK);
            let json_response = result.value.unwrap_or(json!({}));
            (status, Json(json_response)).into_response()
        }
        Err(e) => {
            error!("Error executing DSL: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "error": e.to_string()
            }))).into_response()
        }
    }
}

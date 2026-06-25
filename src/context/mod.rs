use crate::state::StateStore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    variables: Arc<RwLock<HashMap<String, Value>>>,
    request_body: HashMap<String, Value>,
    request_query: HashMap<String, Value>,
    request_headers: HashMap<String, String>,
    request_origin: String,
    project: String,
    state: StateStore,
    /// Identifier of the WS connection that produced the event driving
    /// this DSL run, if any. `Some(id)` for server-side WS frames and
    /// (optionally) source-WS frames; `None` for HTTP and cron-driven
    /// runs. `ws_send` without an explicit `to` uses this id.
    connection_id: Option<String>,
}

impl ExecutionContext {
    pub fn new(
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        origin: String,
    ) -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            request_body: body,
            request_query: query,
            request_headers: headers,
            request_origin: origin,
            project: String::new(),
            state: StateStore::new(),
            connection_id: None,
        }
    }

    /// Constructor used by the router / source dispatchers — binds the
    /// context to a specific project namespace and shares the global
    /// state store.
    pub fn with_state(
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        origin: String,
        project: String,
        state: StateStore,
    ) -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            request_body: body,
            request_query: query,
            request_headers: headers,
            request_origin: origin,
            project,
            state,
            connection_id: None,
        }
    }

    /// Builder: attach a WebSocket connection id to this context.
    /// Used by the WS server (per-client) and (optionally) the WS
    /// source loop so trigger DSLs can `ws_send` back to the upstream.
    pub fn with_connection_id(mut self, id: impl Into<String>) -> Self {
        self.connection_id = Some(id.into());
        self
    }

    pub fn connection_id(&self) -> Option<&str> {
        self.connection_id.as_deref()
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn state(&self) -> &StateStore {
        &self.state
    }

    pub fn set_variable(&self, key: String, value: Value) {
        if let Ok(mut vars) = self.variables.write() {
            vars.insert(key, value);
        }
    }

    pub fn get_variable(&self, key: &str) -> Option<Value> {
        self.variables.read().ok()?.get(key).cloned()
    }

    pub fn get_all_variables(&self) -> HashMap<String, Value> {
        self.variables.read().ok().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn request_body(&self) -> &HashMap<String, Value> {
        &self.request_body
    }

    pub fn request_query(&self) -> &HashMap<String, Value> {
        &self.request_query
    }

    pub fn request_headers(&self) -> &HashMap<String, String> {
        &self.request_headers
    }

    pub fn request_origin(&self) -> &str {
        &self.request_origin
    }
}

use crate::state::StateStore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Task 036 — per-request QuickJS session cache. Holds the Runtime
/// + Context pair together so the Runtime outlives the Context (the
/// Context internally borrows the Runtime). First `evaluate()` call
/// on a request lazily initialises the session; subsequent evaluates
/// in the same request reuse it, avoiding per-call construction of
/// Runtime + Context + JSON binding roundtrip.
///
/// Only available under `scripting-quickjs` because Boa's `Context`
/// is `!Send + !Sync` and cannot cross the `.await` boundaries this
/// field lives across.
#[cfg(feature = "scripting-quickjs")]
pub struct QuickJsSession {
    // Runtime MUST be declared before Context so drop order runs
    // Context first, then Runtime. Otherwise Context's internal
    // reference to Runtime would dangle briefly during drop.
    // (rquickjs internally uses Arc, so this is defensive.)
    pub context: rquickjs::Context,
    pub runtime: rquickjs::Runtime,
}

#[cfg(feature = "scripting-quickjs")]
impl std::fmt::Debug for QuickJsSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickJsSession").finish_non_exhaustive()
    }
}

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
    /// W3C `traceparent` (PATTERNS.md §4). Adopted from the incoming
    /// request when present, otherwise generated at request entry.
    /// http_client forwards it on every outbound call by default.
    traceparent: Option<String>,
    /// Task 036 — lazy per-request QuickJS session. All clones of
    /// this context (e.g. `iterate.do` sub-runs, `template` step
    /// invocations) share the same OnceLock via Arc, so the runtime
    /// is created exactly once per top-level request.
    #[cfg(feature = "scripting-quickjs")]
    quickjs_session: Arc<std::sync::OnceLock<QuickJsSession>>,
}

impl ExecutionContext {
    pub fn new(
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        origin: String,
    ) -> Self {
        let traceparent = headers.get("traceparent").cloned();
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            request_body: body,
            request_query: query,
            request_headers: headers,
            request_origin: origin,
            project: String::new(),
            state: StateStore::new(),
            connection_id: None,
            traceparent,
            #[cfg(feature = "scripting-quickjs")]
            quickjs_session: Arc::new(std::sync::OnceLock::new()),
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
        let traceparent = headers.get("traceparent").cloned();
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            request_body: body,
            request_query: query,
            request_headers: headers,
            request_origin: origin,
            project,
            state,
            connection_id: None,
            traceparent,
            #[cfg(feature = "scripting-quickjs")]
            quickjs_session: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Task 036 — accessor for the per-request QuickJS session
    /// slot. Every clone of this context shares the same
    /// `Arc<OnceLock>`, so `get_or_init`-ing on any clone populates
    /// the slot for all of them. Returns the underlying `Arc` so
    /// the caller (the scripting backend) can `get_or_init` without
    /// re-locking the ExecutionContext.
    #[cfg(feature = "scripting-quickjs")]
    pub fn quickjs_session(&self) -> &Arc<std::sync::OnceLock<QuickJsSession>> {
        &self.quickjs_session
    }

    /// Builder: attach a WebSocket connection id to this context.
    /// Used by the WS server (per-client) and (optionally) the WS
    /// source loop so trigger DSLs can `ws_send` back to the upstream.
    pub fn with_connection_id(mut self, id: impl Into<String>) -> Self {
        self.connection_id = Some(id.into());
        self
    }

    pub fn with_traceparent(mut self, tp: impl Into<String>) -> Self {
        self.traceparent = Some(tp.into());
        self
    }

    pub fn connection_id(&self) -> Option<&str> {
        self.connection_id.as_deref()
    }

    pub fn traceparent(&self) -> Option<&str> {
        self.traceparent.as_deref()
    }

    /// Extract the 32-hex trace id from an adopted traceparent, or return
    /// `None` if we don't have a well-formed one. Used to populate the
    /// `X-Trace-Id` response header.
    pub fn trace_id(&self) -> Option<String> {
        let tp = self.traceparent.as_deref()?;
        // Format: 00-<trace_id 32 hex>-<span_id 16 hex>-<flags 2 hex>
        let parts: Vec<&str> = tp.splitn(4, '-').collect();
        if parts.len() == 4 && parts[1].len() == 32 {
            Some(parts[1].to_string())
        } else {
            None
        }
    }

    /// Explicitly set the traceparent — used by the router when it needs
    /// to generate a fresh one for a request that arrived without a
    /// `traceparent` header.
    pub fn set_traceparent(&mut self, tp: String) {
        self.traceparent = Some(tp);
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

use crate::scripting::ExpressionRegistry;
use crate::state::StateStore;
use crate::{Result, RuuterError};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Issue #79 — hard cap on nested guard invocations. The cycle check
/// in `push_guard` handles the common case (same guard key on the
/// stack). This is belt-and-braces: exotic patterns (e.g. mutual
/// recursion via three different guards) still fail loudly with a
/// clear `RuuterError::DslExecution` instead of an aborting stack
/// overflow. 32 is well beyond any legitimate nested-template
/// composition.
pub const MAX_GUARD_DEPTH: usize = 32;

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
    /// Task 045 — per-session "have I compiled this expression yet"
    /// flags, indexed by the id assigned in `ExpressionRegistry`.
    /// AtomicBool lets us skip the "already compiled?" check
    /// without a Mutex; actual eval is serialised by the underlying
    /// `context.with()` scope regardless. Sized to `registry.len()`
    /// at session construction; `Vec::new()` when registry empty.
    pub compiled_flags: Vec<std::sync::atomic::AtomicBool>,
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
    /// Task 045 — pre-parsed expression registry, built once at
    /// boot from the loaded DSL tree. Cheap to clone (`Arc` inside).
    /// The QuickJS backend consults this at session init to bulk-
    /// compile every registered expression; the Boa backend ignores
    /// it. Empty registry (default) is fine — backends fall back to
    /// per-eval compilation.
    expr_registry: ExpressionRegistry,
    /// Issue #79 — stack of guard keys currently mid-execution.
    /// Guard-loop call sites (HTTP entry, WS upgrade, template step)
    /// push a guard's key before running it and pop after. Nested
    /// `template:` steps consult the stack to filter out guards that
    /// would recurse into themselves (the reporter's project-wide
    /// `.guard.yml` that delegates to a template on a route the
    /// same guard covers).
    ///
    /// Shared via `Arc<Mutex>` so a template step's child
    /// `ExecutionContext` sees the caller's stack (call sites must
    /// invoke `with_guard_stack_from` on the child).
    guard_stack: Arc<Mutex<Vec<String>>>,
}

/// Issue #79 — RAII guard for the `ExecutionContext::guard_stack`.
/// Constructed by `push_guard`; drops (and pops the stack) when it
/// goes out of scope. Two properties this gives us:
///
/// 1. **Panic safety.** A guard-loop iteration that panics or
///    returns Err still pops the entry — no leaked stack frames
///    across request boundaries.
/// 2. **Exception safety at the ?/return sites.** The template
///    step returns early on `>= 400` guard results; the pop still
///    fires because the `_pushed` binding drops at scope exit.
#[must_use = "drop this guard when the enclosed engine.run() returns \
              to pop the guard key off the execution stack"]
pub struct GuardStackGuard {
    stack: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for GuardStackGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardStackGuard").finish_non_exhaustive()
    }
}

impl Drop for GuardStackGuard {
    fn drop(&mut self) {
        // Best-effort pop. A poisoned mutex means an earlier holder
        // panicked; the pop still runs against the poisoned inner
        // state so a subsequent legitimate acquirer sees a coherent
        // (if shorter) stack.
        if let Ok(mut s) = self.stack.lock() {
            s.pop();
        }
    }
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
            expr_registry: ExpressionRegistry::default(),
            guard_stack: Arc::new(Mutex::new(Vec::new())),
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
            expr_registry: ExpressionRegistry::default(),
            guard_stack: Arc::new(Mutex::new(Vec::new())),
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

    /// Task 045 — install the pre-parsed expression registry the
    /// scripting backend will consult. Called by the router when
    /// constructing per-request contexts; the registry itself was
    /// built once at boot from the loaded DSL tree.
    pub fn with_expr_registry(mut self, registry: ExpressionRegistry) -> Self {
        self.expr_registry = registry;
        self
    }

    /// Task 045 — access the pre-parsed expression registry.
    /// Returns the empty default when nothing was installed (e.g.
    /// unit-test contexts built via `ExecutionContext::new`).
    pub fn expr_registry(&self) -> &ExpressionRegistry {
        &self.expr_registry
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
        self.variables
            .read()
            .ok()
            .map(|v| v.clone())
            .unwrap_or_default()
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

    /// Issue #75 — swap the request-side maps in place after guards
    /// have run against the raw request. Guards must see the request
    /// as it hit the wire (headers a guard needs may not be listed in
    /// the route's `declaration.allowlist.headers`); the terminal DSL
    /// then sees the filtered view. Only the router calls this — DSL
    /// steps have no legitimate reason to mutate `incoming.*`.
    pub fn replace_request_view(
        &mut self,
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
    ) {
        self.request_body = body;
        self.request_query = query;
        self.request_headers = headers;
    }

    /// Issue #79 — share the caller's guard stack with a freshly-built
    /// child context. The `template:` step builds its `child_ctx` from
    /// scratch via `ExecutionContext::with_state` rather than cloning;
    /// without this call the child would start with an empty stack and
    /// the recursion detector would miss ancestor guards.
    ///
    /// Cheap — an `Arc::clone` on the shared mutex handle.
    pub fn with_guard_stack_from(mut self, parent: &ExecutionContext) -> Self {
        self.guard_stack = parent.guard_stack.clone();
        self
    }

    /// Issue #79 — true when `key` is on the currently-executing guard
    /// stack. Consulted by every guard-loop call site to skip guards
    /// that would recurse into themselves (project-wide `.guard.yml`
    /// with a `template:` step to a route under the same guard).
    pub fn is_guard_on_stack(&self, key: &str) -> bool {
        self.guard_stack
            .lock()
            .map(|s| s.iter().any(|k| k == key))
            .unwrap_or(false)
    }

    /// Issue #79 — push a guard's key onto the execution stack and
    /// return an RAII drop-guard that pops it. Two hard-error paths:
    ///
    /// - **Cycle.** The key is already on the stack. Returning an
    ///   error here is defence in depth: callers should have already
    ///   filtered against `is_guard_on_stack`, so hitting this branch
    ///   means the filter was skipped (a bug). Errors as
    ///   `DslExecution` so the caller-facing response points at the
    ///   offending guard.
    /// - **Depth cap.** More than `MAX_GUARD_DEPTH` (32) guards on the
    ///   stack. Belt-and-braces for exotic mutual-recursion patterns
    ///   that slip past the cycle check.
    ///
    /// Errors surface as `RuuterError::DslExecution { step: "guard",
    /// message: ... }` — the caller-facing shape a DSL author already
    /// recognises from other guard-related failure modes.
    pub fn push_guard(&self, key: String) -> Result<GuardStackGuard> {
        let mut stack = self
            .guard_stack
            .lock()
            .map_err(|_| RuuterError::DslExecution {
                step: "guard".into(),
                message: "guard stack mutex poisoned".into(),
            })?;
        if stack.len() >= MAX_GUARD_DEPTH {
            return Err(RuuterError::DslExecution {
                step: "guard".into(),
                message: format!(
                    "guard nesting exceeded MAX_GUARD_DEPTH ({}); stack: [{}]. \
                     Likely a `template:` step whose target's guard chain \
                     re-enters via more than {} intermediate guards. See \
                     issue #79.",
                    MAX_GUARD_DEPTH,
                    stack.join(", "),
                    MAX_GUARD_DEPTH,
                ),
            });
        }
        if stack.iter().any(|k| k == &key) {
            return Err(RuuterError::DslExecution {
                step: "guard".into(),
                message: format!(
                    "guard cycle detected: '{}' is already on the execution \
                     stack [{}]. A `template:` step whose target is covered \
                     by the same guard would recurse forever (issue #79).",
                    key,
                    stack.join(", "),
                ),
            });
        }
        stack.push(key);
        Ok(GuardStackGuard {
            stack: self.guard_stack.clone(),
        })
    }

    /// Issue #79 — read-only accessor on the depth. Handy for tests
    /// and any future diagnostic logging.
    pub fn guard_stack_depth(&self) -> usize {
        self.guard_stack.lock().map(|s| s.len()).unwrap_or(0)
    }
}

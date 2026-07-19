use ruuter_on_rust::{
    config::{load_constants, AppConfig},
    dsl::loader::DslLoader,
    http_client::HttpClient,
    observability,
    router::DslRouter,
    scripting::{self, ScriptLimits},
    sources,
    state::StateStore,
    steps::engine::StepEngine,
    supervisor::{self, SourceSupervisor},
    triggers::TriggerDispatcher,
    ws::WsRegistry,
};
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize tracing (with OTel exporter when OTEL_EXPORTER_OTLP_ENDPOINT is set).
    let tracer_provider = observability::init();

    info!("Starting Ruuter-on-Rust v{}", env!("CARGO_PKG_VERSION"));
    if tracer_provider.is_some() {
        info!("OpenTelemetry exporter active");
    }

    // Load configuration (file if resolvable, defaults otherwise)
    let (config, config_source) = match AppConfig::load_or_default() {
        Ok(pair) => pair,
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };
    match &config_source {
        Some(p) => info!("Loaded config from {}", p.display()),
        None => info!("Using built-in default config (no ruuter.yaml found)"),
    }

    // Install script-engine limits before any ScriptEngine::new() runs.
    scripting::install_default_limits(ScriptLimits {
        max_loop_iterations: config.scripting.max_loop_iterations,
        max_stack_size: config.scripting.max_stack_size,
    });

    // Load constants
    let constants = match load_constants("./constants.ini") {
        Ok(c) => {
            info!("Loaded {} constants", c.len());
            c
        }
        Err(e) => {
            error!("Failed to load constants: {}, using empty map", e);
            std::collections::HashMap::new()
        }
    };

    // Load DSLs
    info!("Loading DSL files from {:?}", config.config_path);
    let loader = DslLoader::new(config.clone(), constants.clone());

    let loaded = match loader.load_everything() {
        Ok(d) => {
            let http_total: usize = d.http.values()
                .map(|methods| methods.values().map(|dsls| dsls.len()).sum::<usize>())
                .sum();
            let trigger_total: usize = d.triggers.values().map(|m| m.len()).sum();
            let guard_total: usize = d.guards.values().map(|m| m.len()).sum();
            info!(
                "Loaded {} HTTP DSLs across {} projects, {} trigger DSLs across {} channels, {} guards",
                http_total, d.http.len(), trigger_total, d.triggers.len(), guard_total
            );
            d
        }
        Err(e) => {
            error!("Failed to load DSLs: {}", e);
            std::process::exit(1);
        }
    };

    // Shared state store (project-namespaced k/v) — used by HTTP DSLs
    // today, and by event-trigger DSLs once the WS/cron sources land.
    let state = StateStore::new();

    // Shared WS connection registry. The HTTP router (server-side
    // WS), source supervisor (outbound WS), and step engine
    // (`ws_send` step) all hold a clone of this same Arc-backed map.
    let ws_registry = WsRegistry::new();

    // Wrap the loaded HTTP DSL tree in an Arc so both the router and
    // the step engine can share it without duplicating memory. The
    // engine uses this handle to resolve `template:` step callees at
    // runtime.
    let shared_http_dsls = Arc::new(loaded.http);

    // Shared step engine — same DSL semantics for HTTP routes and
    // event triggers. Carries the WS registry so `ws_send` works.
    // The HttpClient is bound now; its self-call router handle is
    // wired further down, once the DslRouter Arc is available.
    let http_client = HttpClient::new(&config);
    let http_client_for_handle = http_client.clone();
    let mut engine = StepEngine::new(http_client)
        .with_ws_registry(ws_registry.clone())
        .with_dsls(shared_http_dsls.clone());
    if let Some(n) = config.max_step_recursions {
        engine = engine.with_max_iterations(n);
    }

    let trigger_dispatcher = Arc::new(TriggerDispatcher::new(
        loaded.triggers,
        state.clone(),
        engine.clone(),
    ));

    // Sources (WebSocket today; future MQTT/Kafka). Each runs under
    // the SourceSupervisor — panic → exponential-backoff restart with
    // jitter, status visible via /_/sources when admin endpoint is
    // enabled. Scheduled HTTP work is CronManager's job (not Ruuter's).
    let source_configs = sources::loader::load_all(&config).unwrap_or_else(|e| {
        error!("Failed to load source configs: {}", e);
        Vec::new()
    });
    if source_configs.is_empty() {
        info!("No event sources declared");
    } else {
        info!("Spawning {} event source(s) under supervision", source_configs.len());
    }
    let supervisor_arc = Arc::new(SourceSupervisor::new());
    let _source_handles = supervisor::supervise_all(
        source_configs,
        constants,
        trigger_dispatcher,
        ws_registry.clone(),
        supervisor_arc.as_ref(),
    );

    // Build router (which internally generates the OpenAPI spec from the
    // HTTP DSL tree and serves it at GET /_/openapi.json). Shares the
    // same Arc<HttpDsls> the engine uses for template lookup.
    let router = Arc::new(DslRouter::from_arc(
        shared_http_dsls,
        loaded.guards,
        config.clone(),
        state,
        ws_registry,
        engine.clone(),
    ));
    // Task 044 — hand the router handle to HttpClient so http.<verb>
    // targeting our own listener short-circuits back into the router
    // in-process. All existing HttpClient clones share the same
    // OnceCell, so setting once is enough.
    //
    // Bench toggle: `RUUTER_DISABLE_SELF_CALL_SHORTCIRCUIT=true` skips
    // the wiring so an A/B measurement can capture the "without 044"
    // behaviour. Not intended for production — the env var exists so
    // bench harnesses can measure the actual savings the shortcut
    // provides.
    if std::env::var("RUUTER_DISABLE_SELF_CALL_SHORTCIRCUIT")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
    {
        info!("Self-call short-circuit DISABLED via env var (bench mode)");
    } else {
        http_client_for_handle.set_self_call_handler(router.clone());
    }
    let mut app = router.build_axum_router_from_arc();

    // Merge in admin routes when enabled.
    if supervisor::admin_enabled(&config) {
        info!("Admin endpoint enabled at /_/sources");
        app = app.merge(supervisor_arc.clone().admin_router());
    }

    // Task 043 — start server(s). When `config.listeners` is empty,
    // fall back to the 0.4.0 single-TCP-listener behaviour on
    // `config.port`. When non-empty, that config REPLACES the
    // default: every listener spawns its own accept loop; the same
    // axum Router serves all of them.
    if config.listeners.is_empty() {
        let addr = format!("0.0.0.0:{}", config.port);
        info!("Server listening on {}", addr);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|e| {
                error!("Failed to bind to {}: {}", addr, e);
                std::process::exit(1);
            });
        axum::serve(listener, app)
            .await
            .unwrap_or_else(|e| {
                error!("Server error: {}", e);
                std::process::exit(1);
            });
    } else {
        // Multi-listener mode. Each listener runs axum::serve on its
        // own task; the process stays alive until the first listener
        // exits (which normally never happens). Any bind failure at
        // startup is fatal.
        let mut handles = Vec::new();
        for (i, l) in config.listeners.iter().enumerate() {
            let label = l.name.clone().unwrap_or_else(|| format!("listener-{}", i));
            let app = app.clone();
            match (&l.bind, &l.unix) {
                (Some(_), Some(_)) => {
                    error!("listener {}: exactly one of bind/unix required, both set", label);
                    std::process::exit(1);
                }
                (None, None) => {
                    error!("listener {}: exactly one of bind/unix required, neither set", label);
                    std::process::exit(1);
                }
                (Some(bind), None) => {
                    info!("listener {} on TCP {}", label, bind);
                    let listener = tokio::net::TcpListener::bind(bind)
                        .await
                        .unwrap_or_else(|e| {
                            error!("listener {}: bind {} failed: {}", label, bind, e);
                            std::process::exit(1);
                        });
                    handles.push(tokio::spawn(async move {
                        if let Err(e) = axum::serve(listener, app).await {
                            error!("listener {}: {}", label, e);
                        }
                    }));
                }
                (None, Some(path)) => {
                    // Remove stale socket from prior instance.
                    if path.exists() {
                        if let Err(e) = std::fs::remove_file(path) {
                            error!("listener {}: cannot clear stale socket {}: {}", label, path.display(), e);
                            std::process::exit(1);
                        }
                    }
                    info!("listener {} on UDS {}", label, path.display());
                    let listener = tokio::net::UnixListener::bind(path)
                        .unwrap_or_else(|e| {
                            error!("listener {}: bind {} failed: {}", label, path.display(), e);
                            std::process::exit(1);
                        });
                    // axum::serve is TcpListener-only; run a per-
                    // connection hyper HTTP/1 accept loop instead.
                    // The Router is `Clone + Service`, so each
                    // connection gets its own service instance.
                    handles.push(tokio::spawn(async move {
                        loop {
                            let (stream, _addr) = match listener.accept().await {
                                Ok(pair) => pair,
                                Err(e) => {
                                    error!("listener {}: accept error: {}", label, e);
                                    continue;
                                }
                            };
                            let app = app.clone();
                            tokio::spawn(async move {
                                let io = hyper_util::rt::TokioIo::new(stream);
                                let service = hyper_util::service::TowerToHyperService::new(app);
                                if let Err(e) = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(io, service)
                                    .await
                                {
                                    tracing::debug!("uds conn: {}", e);
                                }
                            });
                        }
                    }));
                }
            }
        }
        // Wait for any listener to exit (normally: never). Selecting
        // on all handles rather than joining means one crashing
        // listener brings the process down instead of silently
        // dropping traffic.
        let (result, _idx, _rest) = futures::future::select_all(handles).await;
        if let Err(e) = result {
            error!("listener task panicked: {}", e);
        }
    }

    observability::shutdown(tracer_provider);
}

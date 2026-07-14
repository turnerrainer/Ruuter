use ruuter_rs::{
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

    info!("Starting Ruuter-RS v{}", env!("CARGO_PKG_VERSION"));
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
    let http_client = HttpClient::new(&config);
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
    let router = DslRouter::from_arc(
        shared_http_dsls,
        loaded.guards,
        config.clone(),
        state,
        ws_registry,
        engine.clone(),
    );
    let mut app = router.build_axum_router();

    // Merge in admin routes when enabled.
    if supervisor::admin_enabled(&config) {
        info!("Admin endpoint enabled at /_/sources");
        app = app.merge(supervisor_arc.clone().admin_router());
    }

    // Start server
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

    observability::shutdown(tracer_provider);
}

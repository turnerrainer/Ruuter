use ruuter_rs::{
    config::{load_constants, AppConfig},
    dsl::loader::DslLoader,
    http_client::HttpClient,
    observability,
    router::DslRouter,
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

    // Load configuration
    let config = AppConfig::default();
    info!("Configuration loaded");

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

    // Shared step engine — same DSL semantics for HTTP routes and
    // event triggers. Carries the WS registry so `ws_send` works.
    let http_client = HttpClient::new(config.http_request_timeout);
    let engine = StepEngine::new(http_client).with_ws_registry(ws_registry.clone());

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

    // Build router
    let router = DslRouter::new(
        loaded.http,
        loaded.guards,
        config.clone(),
        state,
        ws_registry,
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

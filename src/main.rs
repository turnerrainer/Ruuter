use ruuter_rs::{
    config::{load_constants, AppConfig},
    dsl::loader::DslLoader,
    router::DslRouter,
};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Ruuter-RS v{}", env!("CARGO_PKG_VERSION"));

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
    let loader = DslLoader::new(config.clone(), constants);

    let dsls = match loader.load_all() {
        Ok(d) => {
            let total: usize = d.values()
                .map(|methods| methods.values().map(|dsls| dsls.len()).sum::<usize>())
                .sum();
            info!("Loaded {} DSL files across {} projects", total, d.len());
            d
        }
        Err(e) => {
            error!("Failed to load DSLs: {}", e);
            std::process::exit(1);
        }
    };

    // Build router
    let router = DslRouter::new(dsls, config.clone());
    let app = router.build_axum_router();

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
}

use arc_swap::ArcSwap;
use ruuter_on_rust::{
    config::{load_constants, AppConfig},
    dsl::{hot_reload, loader::DslLoader},
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
    // Load configuration BEFORE initialising tracing so
    // `logging.format` is honoured. If config load fails, fall back
    // to `eprintln!` because there is no subscriber yet.
    let (config, config_source) = match AppConfig::load_or_default() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize tracing (with OTel exporter when OTEL_EXPORTER_OTLP_ENDPOINT is set).
    let tracer_provider = observability::init(&config);

    info!("Starting Ruuter-on-Rust v{}", env!("CARGO_PKG_VERSION"));
    if tracer_provider.is_some() {
        info!("OpenTelemetry exporter active");
    }
    match &config_source {
        Some(p) => info!("Loaded config from {}", p.display()),
        None => info!("Using built-in default config (no ruuter.yaml found)"),
    }

    // Audit finding 15 — warn at boot when a config field parses but
    // isn't fully honoured. Prevents the silent-drop pattern that let
    // Java operators port `application.yml` verbatim and get a
    // no-op at runtime for fields the framework doesn't wire yet.
    ruuter_on_rust::config::warn_on_stale_config_fields(&config);

    // h2ck.me M2 — flag `RUUTER_HTTP_REWRITE` in release builds. The
    // env var is documented as test-only but the code path is
    // compiled into the release binary; a stray setting in prod
    // silently disables SSRF for the rewritten origin. Warn loudly
    // so a misconfiguration shows up in the same log stream as
    // "Loaded config from …" and hooks operators before the first
    // outbound request fires.
    if ruuter_on_rust::http_client::rewrite_env_is_active_in_release() {
        tracing::warn!(
            env = ruuter_on_rust::http_client::RUUTER_HTTP_REWRITE_ENV,
            "RUUTER_HTTP_REWRITE is set in a release build — every outbound URL \
             whose origin matches a `from` prefix is silently rewritten BEFORE \
             SSRF checks (allowlists, block_private_networks). Intended for \
             test harnesses only. Unset it in production."
        );
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
            let http_total: usize = d
                .http
                .values()
                .map(|methods| methods.values().map(|dsls| dsls.len()).sum::<usize>())
                .sum();
            let trigger_total: usize = d.triggers.values().map(|m| m.len()).sum();
            let guard_total: usize = d.guards.values().map(|m| m.len()).sum();
            info!(
                "Loaded {} HTTP DSLs across {} projects, {} trigger DSLs across {} channels, {} guards",
                http_total, d.http.len(), trigger_total, d.triggers.len(), guard_total
            );
            // Task 070 — surface DSLs without a declaration block.
            // Never fatal; just an operator signal. Gate on the
            // config toggle (default on) so noisy corpora can silence it.
            let missing_decl = ruuter_on_rust::dsl::loader::warn_on_missing_declarations(
                &d.http,
                config.dsl.warn_on_missing_declaration,
            );
            if missing_decl > 0 && config.dsl.warn_on_missing_declaration {
                info!(
                    "{} of {} HTTP DSLs have no declaration block; add `declaration:` \
                     for richer OpenAPI + strict-key support (see book/src/dsl/declaration.md), \
                     or set `dsl.warn_on_missing_declaration: false` to hide these WARNs.",
                    missing_decl, http_total
                );
            }
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

    // Wrap the loaded HTTP DSL tree + guards in ArcSwaps so both the
    // router and the step engine can share them without duplicating
    // memory — and so the hot-reload watcher can atomically swap the
    // published tree without stopping in-flight requests. The engine
    // uses the HTTP handle to resolve `template:` step callees at
    // runtime; both handles are handed to `DslRouter::from_shared`.
    let shared_http_dsls: ruuter_on_rust::dsl::loader::SharedHttpDsls =
        Arc::new(ArcSwap::from_pointee(loaded.http));
    let shared_guards: ruuter_on_rust::dsl::loader::SharedGuards =
        Arc::new(ArcSwap::from_pointee(loaded.guards));

    // Task 045 — pre-parsed expression registry, built once at
    // boot by walking every HTTP DSL, guard, and trigger DSL for
    // `${...}` / `$=...=` expressions. Scripting backends (only
    // QuickJS today) consult this at session init to bulk-compile
    // every expression; Boa ignores it.
    let expr_registry = {
        let mut b = ruuter_on_rust::scripting::registry::Builder::new();
        // Snapshot the current tree for boot-time bulk-compile. Hot
        // reloads publish new trees at runtime; QuickJS re-scanning
        // that registry per-reload is out of scope for the first
        // hot-reload cut (documented in dsl/hot_reload.rs).
        b.add_http(&shared_http_dsls.load());
        b.add_guards(&shared_guards.load());
        b.add_trigger_dsls(&loaded.triggers);
        b.freeze()
    };
    info!(
        "Pre-parsed expression registry: {} unique JS expressions",
        expr_registry.len()
    );

    // Shared step engine — same DSL semantics for HTTP routes and
    // event triggers. Carries the WS registry so `ws_send` works.
    // The HttpClient is bound now; its self-call router handle is
    // wired further down, once the DslRouter Arc is available.
    let http_client = HttpClient::new(&config);
    let http_client_for_handle = http_client.clone();
    let logging_arc = Arc::new(config.logging.clone());
    let mut engine = StepEngine::new(http_client)
        .with_ws_registry(ws_registry.clone())
        // `with_dsls_shared` (not `with_dsls`) so the engine and the
        // router below observe the *same* ArcSwap. Without this, a
        // hot-reload publish on the router would leave the engine's
        // template-lookup handle pointing at the stale tree.
        .with_dsls_shared(shared_http_dsls.clone())
        // h2ck.me H1 — share the guards ArcSwap with the engine so
        // the `template:` step enforces the same guard chain the
        // HTTP entry path runs. Skipping this would leave a public
        // DSL free to template into a guarded admin route.
        .with_guards(shared_guards.clone(), config.guards.mode)
        .with_expr_registry(expr_registry)
        .with_logging(logging_arc.clone());
    if let Some(n) = config.max_step_recursions {
        engine = engine.with_max_iterations(n);
    }
    // Audit finding 13 — install the default exception DSL config
    // if the operator set one. HttpStepExecutor invokes it on
    // upstream error when no local `error:` handler is set.
    if let Some(cfg) = config.default_dsl_in_case_of_exception.clone() {
        engine = engine.with_default_exception_dsl(cfg);
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
        info!(
            "Spawning {} event source(s) under supervision",
            source_configs.len()
        );
    }
    let supervisor_arc = Arc::new(SourceSupervisor::new());
    let _source_handles = supervisor::supervise_all(
        source_configs,
        constants.clone(),
        trigger_dispatcher,
        ws_registry.clone(),
        supervisor_arc.as_ref(),
    );

    // Build router (which internally generates the OpenAPI spec from the
    // HTTP DSL tree and serves it at GET /_/openapi.json). Shares the
    // same ArcSwap-wrapped handles the engine uses for template lookup
    // so a single hot-reload publish is visible to both.
    let router = Arc::new(DslRouter::from_shared(
        shared_http_dsls.clone(),
        shared_guards.clone(),
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

    // Audit finding 01: install the step-driven reload handler.
    // Every clone of the engine sees the same OnceCell slot, so
    // one set() is enough. Handler itself gates on
    // `dsl.allow_dsl_reloading` when the step fires — matches
    // Java's "not enabled in configuration" log-and-drop.
    engine.set_reload_handler(std::sync::Arc::new(
        ruuter_on_rust::dsl::hot_reload::StepReloadHandler::new(
            config.clone(),
            constants.clone(),
            router.clone(),
        ),
    ));

    // DSL hot-reload watcher (dev-only). Off by default. See the
    // module docstring for security posture and the exhaustive list
    // of what does / does not reload.
    if config.dsl.allow_dsl_reloading {
        if let Err(e) = hot_reload::spawn(config.clone(), constants.clone(), router.clone()) {
            error!("Failed to start DSL hot-reload watcher: {}", e);
        }
    }

    let mut app = router.clone().build_axum_router_from_arc();

    // Merge in admin routes when enabled.
    if supervisor::admin_enabled(&config) {
        info!("Admin endpoints enabled: /_/sources, /_/unguarded");
        app = app.merge(supervisor_arc.clone().admin_router());
        app = app.merge(router.admin_router());
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
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
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
                    error!(
                        "listener {}: exactly one of bind/unix required, both set",
                        label
                    );
                    std::process::exit(1);
                }
                (None, None) => {
                    error!(
                        "listener {}: exactly one of bind/unix required, neither set",
                        label
                    );
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
                        if let Err(e) = axum::serve(
                            listener,
                            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                        )
                        .await
                        {
                            error!("listener {}: {}", label, e);
                        }
                    }));
                }
                (None, Some(path)) => {
                    // Remove stale socket from prior instance.
                    if path.exists() {
                        if let Err(e) = std::fs::remove_file(path) {
                            error!(
                                "listener {}: cannot clear stale socket {}: {}",
                                label,
                                path.display(),
                                e
                            );
                            std::process::exit(1);
                        }
                    }
                    let http_version = if l.http2 { "h2c" } else { "http/1.1" };
                    info!(
                        "listener {} on UDS {} ({})",
                        label,
                        path.display(),
                        http_version
                    );
                    let listener = tokio::net::UnixListener::bind(path).unwrap_or_else(|e| {
                        error!("listener {}: bind {} failed: {}", label, path.display(), e);
                        std::process::exit(1);
                    });
                    let use_h2 = l.http2;
                    // axum::serve is TcpListener-only; run a per-
                    // connection hyper accept loop instead. The
                    // Router is `Clone + Service`, so each connection
                    // gets its own service instance. Task 049 adds
                    // h2c via the http2 server builder as an alt
                    // path controlled by `listener.http2: true`.
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
                                if use_h2 {
                                    if let Err(e) = hyper::server::conn::http2::Builder::new(
                                        hyper_util::rt::TokioExecutor::new(),
                                    )
                                    .serve_connection(io, service)
                                    .await
                                    {
                                        tracing::debug!("uds h2c conn: {}", e);
                                    }
                                } else if let Err(e) = hyper::server::conn::http1::Builder::new()
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

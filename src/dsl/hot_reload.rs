//! DSL hot-reload watcher.
//!
//! When `config.dsl.allow_dsl_reloading == true`, `spawn` starts a
//! filesystem watcher over `config.config_path` (the DSL root).
//! Filesystem events are debounced (300 ms) so a bulk editor save or
//! `git checkout` triggers one reload, not dozens. On each debounced
//! event the watcher re-runs `DslLoader::load_everything()` against
//! the boot-time constants map and — on success — atomically publishes
//! the new HTTP and guard trees through `DslRouter::publish_dsls`.
//!
//! ## Scope
//!
//! - HTTP DSLs are hot-reloaded (`DSL/<project>/<METHOD>/*.yml`).
//! - Guards are hot-reloaded.
//! - The OpenAPI cache is rebuilt from the new tree.
//! - The `template:` step sees the new tree because the engine's
//!   handle and the router's handle point at the same `ArcSwap`.
//!
//! ## Explicitly NOT reloaded (documented as such)
//!
//! - **Trigger DSLs** (`DSL/<project>/triggers/**`) — the trigger
//!   dispatcher is built once at boot and holds its own owned map.
//! - **Source configs** (`DSL/<project>/sources/**`) — the source
//!   supervisor spawns each source once; reloading would require
//!   graceful teardown of live WebSocket connections.
//! - **`constants.ini`** — constants are baked into DSLs at parse
//!   time. Changing a constant value needs a restart.
//! - **`ruuter.yaml`** — operator config file. Restart required for
//!   any change (bindings, CORS, SSRF, script limits, ...).
//! - **Pre-parsed expression registry** — QuickJS backend snapshots
//!   the registry at session init. New `${...}` expressions added
//!   after reload will fall back to per-eval compilation on QuickJS
//!   (Boa always compiles per-eval; unaffected).
//!
//! Reload failures (parse error, missing constant, broken step graph)
//! are logged at WARN and the previously-published tree stays live —
//! a broken save cannot take the server down.
//!
//! ## Security posture
//!
//! Hot-reload combined with a writable `DSL/` mount is effectively
//! remote code execution via `${JS}` expressions. Ship with
//! `dsl.allow_dsl_reloading: false` and a read-only DSL mount in
//! production; enable only for local development. The shipped
//! `docker-compose.yml` mounts `./DSL:/app/DSL:ro` and sets
//! `read_only: true` on the container filesystem — both defaults hold
//! even when hot-reload is enabled.

use crate::config::AppConfig;
use crate::dsl::loader::DslLoader;
use crate::router::DslRouter;
use notify::RecursiveMode;
use notify_debouncer_full::new_debouncer;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Debounce window. A single "Save All" in an editor can produce ~10
/// per-file events within a few ms; a git checkout of a feature
/// branch can produce hundreds. 300 ms coalesces those into one
/// reload while staying fast enough to feel instant to a developer
/// editing one file at a time.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);

/// Spawn the hot-reload watcher. Returns `Ok(())` once the watcher
/// task is registered; the task itself runs for the lifetime of the
/// process.
///
/// The watcher takes ownership of a boot-time snapshot of the
/// constants map (since `constants.ini` is not reloaded — see the
/// module docstring) and a shared handle to the router so it can
/// publish new trees.
pub fn spawn(
    config: AppConfig,
    constants: HashMap<String, String>,
    router: Arc<DslRouter>,
) -> notify::Result<()> {
    let dsl_root = config.config_path.clone();
    if !dsl_root.exists() {
        warn!(
            "hot-reload requested but DSL root {} does not exist — watcher not started",
            dsl_root.display()
        );
        return Ok(());
    }

    // Channel between the (blocking) notify thread and the tokio
    // consumer task. Bounded so a runaway event storm can't grow the
    // queue without limit; on backpressure the notify thread will
    // drop events, which is fine because each successful reload
    // observes the *current* filesystem state anyway.
    let (tx, mut rx) = mpsc::channel::<()>(16);

    // Debouncer coalesces bursty filesystem events (editor saves,
    // git checkouts) into a single "something changed under DSL/"
    // signal. We don't care which file changed — we always re-scan
    // the whole tree, since a single-file diff would miss deletes
    // and renames.
    let mut debouncer = new_debouncer(DEBOUNCE_WINDOW, None, move |res| match res {
        Ok(_events) => {
            // Non-blocking send; drop on backpressure (harmless
            // because the consumer always re-scans from disk).
            let _ = tx.try_send(());
        }
        Err(errs) => {
            for e in errs {
                warn!("hot-reload watcher: {}", e);
            }
        }
    })?;

    debouncer.watch(&dsl_root, RecursiveMode::Recursive)?;

    info!(
        "hot-reload: watching {} (debounce {} ms)",
        dsl_root.display(),
        DEBOUNCE_WINDOW.as_millis()
    );

    tokio::spawn(async move {
        // Keep the debouncer alive for the lifetime of the task —
        // dropping it would tear down the notify watch thread.
        let _keepalive = debouncer;
        while rx.recv().await.is_some() {
            reload_once(&config, &constants, &router);
        }
    });

    Ok(())
}

fn reload_once(config: &AppConfig, constants: &HashMap<String, String>, router: &DslRouter) {
    debug!("hot-reload: re-scanning DSL tree");
    let loader = DslLoader::new(config.clone(), constants.clone());
    match loader.load_everything() {
        Ok(loaded) => {
            let http_total: usize = loaded
                .http
                .values()
                .map(|methods| methods.values().map(|dsls| dsls.len()).sum::<usize>())
                .sum();
            let guard_total: usize = loaded.guards.values().map(|m| m.len()).sum();
            router.publish_dsls(loaded.http, loaded.guards);
            info!(
                "hot-reload: republished {} HTTP DSL(s), {} guard(s)",
                http_total, guard_total
            );
        }
        Err(e) => {
            // Keep serving the previously-published tree. A broken
            // save cannot take the server down.
            error!(
                "hot-reload: reload failed (previous tree still live): {}",
                e
            );
        }
    }
}

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
//!
//! ## Event filtering (audit finding 02, fixed)
//!
//! The debouncer subscribes to inotify by default, which on Linux
//! includes `IN_ATTRIB` events. Under `strictatime` mount options
//! (some Docker/K8s CSI drivers, some NFS setups), each read updates
//! the file's `atime`, which fires an `IN_ATTRIB` event. Without
//! filtering, that meant `read_to_string` during a reload triggered
//! another reload → infinite loop.
//!
//! Now we filter debounced events by both kind and path:
//!
//! - **Kind filter** — reload only on `Create`, `Remove`, or
//!   `Modify(Data | Name)`. Access events and pure metadata /
//!   attribute changes (permission, atime, owner) do not trigger a
//!   reload.
//! - **Path filter** — reload only when at least one event path has
//!   a DSL-relevant extension (`.yml`, `.yaml`, or a `.guard`-shaped
//!   filename). A `.git/HEAD` touch, a swap-file rename, a
//!   `.DS_Store` write — all pass through without a reload.
//! - **Reserved-subdir skip** — events entirely inside `triggers/`,
//!   `sources/`, or `cronmanager-jobs/` do not trigger a reload
//!   because those aren't part of the hot-reloaded HTTP+guard tree.
//!
//! The reload itself now runs inside `tokio::task::spawn_blocking`
//! so the sync `fs::read_dir` / `fs::read_to_string` walk doesn't
//! stall the runtime worker.

use crate::config::AppConfig;
use crate::dsl::loader::DslLoader;
use crate::router::DslRouter;
use async_trait::async_trait;
use notify::event::{EventKind, ModifyKind};
use notify::RecursiveMode;
use notify_debouncer_full::new_debouncer;
use notify_debouncer_full::DebouncedEvent;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Audit finding 01 — implements `steps::engine::ReloadHandler` on
/// the same call chain the filesystem watcher uses. When a DSL step
/// with `reload_dsl: true` runs, the engine invokes
/// `StepReloadHandler::trigger_reload`; that dispatches into
/// [`reload_once`] on a blocking task, gated on
/// `config.dsl.allow_dsl_reloading`.
///
/// Constructing one requires the same triple as the watcher: the
/// current config, the boot-time constants snapshot, and the router
/// handle to publish onto. Main wires it before building the engine.
pub struct StepReloadHandler {
    config: AppConfig,
    constants: HashMap<String, String>,
    router: Arc<DslRouter>,
}

impl StepReloadHandler {
    pub fn new(
        config: AppConfig,
        constants: HashMap<String, String>,
        router: Arc<DslRouter>,
    ) -> Self {
        Self {
            config,
            constants,
            router,
        }
    }
}

#[async_trait]
impl crate::steps::engine::ReloadHandler for StepReloadHandler {
    async fn trigger_reload(&self) {
        // Java: "Only allow reloading if it's enabled in
        // configuration." Log at ERROR (matches Java's log-and-drop
        // when the gate is off) so operators can see when a DSL
        // step is asking for a reload it can't get.
        if !self.config.dsl.allow_dsl_reloading {
            tracing::error!(
                "reloadDsl step ran but dsl.allow_dsl_reloading is off — reload NOT performed"
            );
            return;
        }
        let config = self.config.clone();
        let constants = self.constants.clone();
        let router = self.router.clone();
        let join = tokio::task::spawn_blocking(move || {
            reload_once(&config, &constants, &router);
        });
        if let Err(e) = join.await {
            tracing::error!("reloadDsl step: reload task panicked: {}", e);
        }
    }
}

/// Debounce window. A single "Save All" in an editor can produce ~10
/// per-file events within a few ms; a git checkout of a feature
/// branch can produce hundreds. 300 ms coalesces those into one
/// reload while staying fast enough to feel instant to a developer
/// editing one file at a time.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);

/// Subdirs under a project that are NOT part of the hot-reloaded
/// HTTP + guard tree (see module docstring). Events entirely under
/// one of these do not trigger a reload.
const RESERVED_SUBDIRS: &[&str] = &["triggers", "sources", "cronmanager-jobs"];

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

    let dsl_root_for_filter = dsl_root.clone();

    // Debouncer coalesces bursty filesystem events (editor saves,
    // git checkouts) into a single "something changed under DSL/"
    // signal. We inspect each debounced batch and only forward when
    // at least ONE event has a kind + path we care about; see
    // `event_warrants_reload`. Ignoring the whole batch on no-match
    // is what closes the atime-loop hole from audit finding 02.
    let mut debouncer = new_debouncer(
        DEBOUNCE_WINDOW,
        None,
        move |res: notify_debouncer_full::DebounceEventResult| match res {
            Ok(events) => {
                if batch_warrants_reload(&events, &dsl_root_for_filter) {
                    // Non-blocking send; drop on backpressure (harmless
                    // because the consumer always re-scans from disk).
                    let _ = tx.try_send(());
                } else {
                    debug!(
                        "hot-reload: skipping batch of {} event(s) — no DSL-relevant paths",
                        events.len()
                    );
                }
            }
            Err(errs) => {
                for e in errs {
                    warn!("hot-reload watcher: {}", e);
                }
            }
        },
    )?;

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
            // Sync fs walk moved off the tokio worker via
            // spawn_blocking so a large DSL tree can't stall other
            // async tasks during the reload. `join.await` awaits
            // completion; errors are logged.
            let config = config.clone();
            let constants = constants.clone();
            let router = router.clone();
            let join = tokio::task::spawn_blocking(move || {
                reload_once(&config, &constants, &router);
            });
            if let Err(e) = join.await {
                error!("hot-reload: reload task panicked: {}", e);
            }
        }
    });

    Ok(())
}

/// Return `true` when at least one event in the debounced batch is
/// (a) a content-changing kind AND (b) references a DSL-relevant
/// path. Batches where every event fails either half of that test
/// are dropped without a reload.
///
/// The kind gate rejects `Access(_)`, `Modify(Metadata)`,
/// `Modify(Any)`, and `Modify(Other)` — the four categories that
/// fire on `chmod`, `touch`, and (crucially) `atime` updates on
/// `strictatime` mounts. That closes the reload loop from finding 02.
fn batch_warrants_reload(events: &[DebouncedEvent], dsl_root: &Path) -> bool {
    events
        .iter()
        .any(|ev| event_kind_matters(&ev.event.kind) && event_paths_relevant(&ev.event.paths, dsl_root))
}

fn event_kind_matters(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Name(_))
    )
}

fn event_paths_relevant(paths: &[std::path::PathBuf], dsl_root: &Path) -> bool {
    if paths.is_empty() {
        // Some backends emit events without paths (rescan, generic
        // watch errors). Assume relevance rather than lose signal.
        return true;
    }
    paths.iter().any(|p| path_is_relevant(p, dsl_root))
}

fn path_is_relevant(path: &Path, dsl_root: &Path) -> bool {
    if is_under_reserved_subdir(path, dsl_root) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // Guard files (Java `.guard` convention, or `.guard.yml`, or a
    // sibling `<stem>.guard.<ext>`) count as relevant.
    if name == ".guard" || name.contains(".guard.") {
        return true;
    }
    // YAML DSLs.
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yml") | Some("yaml")
    )
}

/// True when `path` lives (transitively) under `<dsl_root>/<project>/<reserved>/…`
/// for any reserved subdir name, OR under `<dsl_root>/<project>/WS/outbound/…`
/// (the new-layout equivalent of legacy `sources/`). Guards against firing
/// HTTP-tree reloads for edits that only affect outbound feeds or triggers.
fn is_under_reserved_subdir(path: &Path, dsl_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(dsl_root) else {
        return false;
    };
    let comps: Vec<_> = rel.components().collect();
    // Layout: <project>/<reserved>/… — reserved subdir is the
    // SECOND component under the DSL root.
    if comps.len() < 2 {
        return false;
    }
    let second = comps.get(1).and_then(|c| c.as_os_str().to_str());
    if let Some(name) = second {
        if RESERVED_SUBDIRS.contains(&name) {
            return true;
        }
        // WS/outbound/ — new-layout equivalent of legacy sources/.
        // WS/inbound/ is NOT reserved (it drives the HTTP tree).
        if name == "WS" {
            let third = comps.get(2).and_then(|c| c.as_os_str().to_str());
            if third == Some("outbound") {
                return true;
            }
        }
    }
    false
}

pub(crate) fn reload_once(config: &AppConfig, constants: &HashMap<String, String>, router: &DslRouter) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, CreateKind, DataChange, Event, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };
    use std::path::PathBuf;

    fn debounced(kind: EventKind, paths: Vec<PathBuf>) -> DebouncedEvent {
        let mut ev = Event::new(kind);
        ev.paths = paths;
        DebouncedEvent::new(ev, instant::Instant::now())
    }

    fn dsl_root() -> PathBuf {
        PathBuf::from("/tmp/dsl-test-root")
    }

    // Reload TRIGGERS

    #[test]
    fn modify_data_on_yaml_triggers() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn create_yaml_triggers() {
        let ev = debounced(
            EventKind::Create(CreateKind::File),
            vec![dsl_root().join("proj/GET/new.yaml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn remove_yaml_triggers() {
        let ev = debounced(
            EventKind::Remove(RemoveKind::File),
            vec![dsl_root().join("proj/GET/gone.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn rename_yaml_triggers() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            vec![dsl_root().join("proj/GET/renamed.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn create_guard_dot_file_triggers() {
        let ev = debounced(
            EventKind::Create(CreateKind::File),
            vec![dsl_root().join("proj/GET/nested/.guard")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn sibling_dot_guard_dot_yml_triggers() {
        let ev = debounced(
            EventKind::Create(CreateKind::File),
            vec![dsl_root().join("proj/GET/foo.guard.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    // Reload SKIPS — this is the finding-02 core

    #[test]
    #[allow(non_snake_case)]
    fn metadata_modify_on_yaml_does_NOT_trigger() {
        // strictatime scenario: reading a .yml file updates atime →
        // IN_ATTRIB → ModifyKind::Metadata. Would loop if allowed.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn access_open_does_NOT_trigger() {
        let ev = debounced(
            EventKind::Access(AccessKind::Open(notify::event::AccessMode::Read)),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_any_does_NOT_trigger_on_yaml_path() {
        // Modify::Any is the catch-all imprecise kind — some backends
        // use it for attribute changes. Fail closed.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Any),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_on_non_yaml_does_NOT_trigger() {
        // Editor swap file — content-change event, but not a DSL.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/GET/.hello.yml.swp")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_on_dot_ds_store_does_NOT_trigger() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/GET/.DS_Store")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_on_git_head_does_NOT_trigger() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join(".git/HEAD")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_under_triggers_subdir_does_NOT_trigger() {
        // triggers/ is explicitly NOT hot-reloaded (see docstring).
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/triggers/topic/on-event.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_under_sources_subdir_does_NOT_trigger() {
        // Legacy sources/ layout — still ignored by hot-reload.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/sources/stock-feed.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_under_ws_outbound_does_NOT_trigger() {
        // New-layout equivalent of sources/. Same treatment.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/WS/outbound/stock-feed.yml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn modify_data_under_ws_inbound_triggers() {
        // WS/inbound/ is part of the HTTP tree — reload MUST fire.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/WS/inbound/echo.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    fn modify_data_under_ws_legacy_direct_triggers() {
        // Legacy WS/<file>.yml layout — still triggers reload.
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/WS/echo.yml")],
        );
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }

    #[test]
    #[allow(non_snake_case)]
    fn modify_data_under_cronmanager_does_NOT_trigger() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/cronmanager-jobs/heartbeat.yaml")],
        );
        assert!(!batch_warrants_reload(&[ev], &dsl_root()));
    }

    // Batches: mixed events → any single relevant event triggers.

    #[test]
    fn batch_with_one_relevant_and_many_irrelevant_triggers() {
        let irrelevant = debounced(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        let relevant = debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![dsl_root().join("proj/POST/new.yml")],
        );
        assert!(batch_warrants_reload(
            &[irrelevant.clone(), irrelevant, relevant],
            &dsl_root()
        ));
    }

    #[test]
    #[allow(non_snake_case)]
    fn batch_of_only_metadata_events_does_NOT_trigger() {
        let ev = debounced(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
            vec![dsl_root().join("proj/GET/hello.yml")],
        );
        assert!(!batch_warrants_reload(
            &[ev.clone(), ev.clone(), ev],
            &dsl_root()
        ));
    }

    // Edge case: event with empty path list is treated as relevant
    // (rescan / generic errors). We prefer false positives to lost signal.

    #[test]
    fn event_without_paths_treated_as_relevant() {
        let ev = debounced(EventKind::Create(CreateKind::File), vec![]);
        assert!(batch_warrants_reload(&[ev], &dsl_root()));
    }
}

//! Generic event sources.
//!
//! A "source" is a long-lived background task that receives messages
//! from somewhere outside Ruuter (a WebSocket, a cron schedule, a
//! file watcher, …) and dispatches each one to the
//! `TriggerDispatcher`. Ruuter knows nothing about the meaning of the
//! messages — services declare what to do per (channel, key) via
//! trigger DSLs.
//!
//! Each project's sources live under:
//!
//! ```text
//! DSL/<project>/sources/<source_name>.yml
//! ```

pub mod config;
pub mod loader;
pub mod ws;

use crate::triggers::TriggerDispatcher;
use crate::ws::WsRegistry;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Spawn every source declared in `configs` as a background task.
/// Returns the join handles so callers can keep references for shutdown.
pub fn spawn_all(
    configs: Vec<(String, String, config::SourceConfig)>, // (project, source_name, config)
    constants: std::collections::HashMap<String, String>,
    dispatcher: Arc<TriggerDispatcher>,
    registry: WsRegistry,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::with_capacity(configs.len());
    for (project, name, cfg) in configs {
        match cfg {
            config::SourceConfig::WebSocket(ws_cfg) => {
                let resolved = match config::resolve_constants(&ws_cfg, &constants) {
                    Ok(r) => r,
                    Err(e) => {
                        error!(%project, %name, "source config constant resolution failed: {}", e);
                        continue;
                    }
                };
                let dispatcher = dispatcher.clone();
                let registry = registry.clone();
                let project_for_task = project.clone();
                let name_for_task = name.clone();
                info!(%project, %name, url = %resolved.url, "spawning websocket source");
                handles.push(tokio::spawn(async move {
                    ws::run(
                        project_for_task,
                        name_for_task,
                        resolved,
                        dispatcher,
                        registry,
                    )
                    .await;
                }));
            }
        }
    }
    handles
}

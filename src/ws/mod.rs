//! WebSocket connection registry — the shared sink Bus that
//! `ws_send` writes into and that both the WS server (inbound
//! per-client connections) and WS sources (outbound upstream
//! connections) register against.
//!
//! Every writable WS connection is owned by a single writer task that
//! drains an `mpsc::UnboundedReceiver<Value>` and serializes each
//! `Value` as a `Text` frame on its socket. The matching
//! `UnboundedSender<Value>` is held in this registry keyed by an
//! arbitrary connection id (string). A DSL invokes
//! `ws_send: { to: "<id>", payload: ... }` and the registry forwards
//! the payload to the right writer.
//!
//! Connection id convention (Ruuter does not enforce it; just a
//! suggested namespace pattern):
//!   - `client:<random-hex>` — an inbound server-side WS client
//!   - `source:<project>:<source_name>` — an outbound source WS
//!
//! The registry is `Clone` (cheap, internally `Arc<DashMap>`), so the
//! engine, router, and source supervisor can each carry a handle.

use crate::{Result, RuuterError};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

pub type ConnectionId = String;

/// Outbound message envelope. Today only JSON Text is supported; this
/// enum exists so we can add Binary / Close later without breaking
/// the registry signature.
#[derive(Debug, Clone)]
pub enum Outbound {
    Json(Value),
}

#[derive(Clone, Default, Debug)]
pub struct WsRegistry {
    inner: Arc<DashMap<ConnectionId, mpsc::UnboundedSender<Outbound>>>,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, id: ConnectionId, sender: mpsc::UnboundedSender<Outbound>) {
        self.inner.insert(id, sender);
    }

    pub fn unregister(&self, id: &str) {
        self.inner.remove(id);
    }

    pub fn send(&self, id: &str, payload: Value) -> Result<()> {
        let entry = self
            .inner
            .get(id)
            .ok_or_else(|| RuuterError::InvalidStep(format!("ws_send: no such connection '{}'", id)))?;
        entry
            .send(Outbound::Json(payload))
            .map_err(|e| RuuterError::InvalidStep(format!("ws_send: writer dropped for '{}': {}", id, e)))?;
        Ok(())
    }

    /// Broadcast to every connection whose id satisfies `pred`. Returns
    /// the number of connections that accepted the message.
    pub fn broadcast<F>(&self, pred: F, payload: Value) -> usize
    where
        F: Fn(&str) -> bool,
    {
        let mut delivered = 0;
        for entry in self.inner.iter() {
            if pred(entry.key()) {
                if entry.value().send(Outbound::Json(payload.clone())).is_ok() {
                    delivered += 1;
                }
            }
        }
        delivered
    }

    pub fn connection_count(&self) -> usize {
        self.inner.len()
    }

    pub fn ids(&self) -> Vec<ConnectionId> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }
}

/// Generate a random hex connection id. Used by the server-side WS
/// upgrade handler when a new client connects.
pub fn random_client_id() -> ConnectionId {
    use rand::Rng;
    let id: u128 = rand::thread_rng().gen();
    format!("client:{:032x}", id)
}

/// Build the conventional source id from a project + source name.
pub fn source_id(project: &str, source_name: &str) -> ConnectionId {
    format!("source:{}:{}", project, source_name)
}

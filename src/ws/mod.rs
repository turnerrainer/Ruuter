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
//! ## Connection tags
//!
//! Every connection also carries a small string→string tag map. A WS
//! server DSL sets tags on the originating connection with the
//! `ws_tag` step — typically stamping the authenticated identity
//! (`user`, `roles`, `tenant`, …) it just resolved on connect. Any
//! later `ws_send` can then fan out to exactly the connections whose
//! tags match (`ws_send: { broadcast_where: { tag: "roles",
//! contains: "admin" }, … }`), without the DSL author standing up an
//! external directory of "which socket belongs to whom". Tags never
//! leave the process and are dropped when the connection unregisters.
//!
//! The registry is `Clone` (cheap, internally `Arc<DashMap>`), so the
//! engine, router, and source supervisor can each carry a handle.

use crate::{Result, RuuterError};
use dashmap::DashMap;
use serde_json::Value;
use std::collections::HashMap;
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

/// One registered connection: its writer channel plus the mutable
/// tag map a DSL stamps on it via `ws_tag`.
#[derive(Debug)]
struct Connection {
    tx: mpsc::UnboundedSender<Outbound>,
    tags: DashMap<String, String>,
}

#[derive(Clone, Default, Debug)]
pub struct WsRegistry {
    inner: Arc<DashMap<ConnectionId, Connection>>,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, id: ConnectionId, sender: mpsc::UnboundedSender<Outbound>) {
        self.inner.insert(
            id,
            Connection {
                tx: sender,
                tags: DashMap::new(),
            },
        );
    }

    pub fn unregister(&self, id: &str) {
        self.inner.remove(id);
    }

    pub fn send(&self, id: &str, payload: Value) -> Result<()> {
        let entry = self.inner.get(id).ok_or_else(|| {
            RuuterError::InvalidStep(format!("ws_send: no such connection '{}'", id))
        })?;
        entry.tx.send(Outbound::Json(payload)).map_err(|e| {
            RuuterError::InvalidStep(format!("ws_send: writer dropped for '{}': {}", id, e))
        })?;
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
            if pred(entry.key()) && entry.value().tx.send(Outbound::Json(payload.clone())).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// Broadcast to every connection whose `(id, tags)` satisfy `pred`.
    /// The tag map is snapshotted per connection so the predicate sees
    /// a stable view. Returns the number of connections that accepted
    /// the message.
    pub fn broadcast_where<F>(&self, pred: F, payload: Value) -> usize
    where
        F: Fn(&str, &HashMap<String, String>) -> bool,
    {
        let mut delivered = 0;
        for entry in self.inner.iter() {
            let tags: HashMap<String, String> = entry
                .value()
                .tags
                .iter()
                .map(|t| (t.key().clone(), t.value().clone()))
                .collect();
            if pred(entry.key(), &tags)
                && entry.value().tx.send(Outbound::Json(payload.clone())).is_ok()
            {
                delivered += 1;
            }
        }
        delivered
    }

    /// Merge `entries` into the tag map of connection `id`. Existing
    /// keys are overwritten; keys not mentioned are left untouched.
    /// Errors if the connection is not (or no longer) registered.
    pub fn set_tags<I>(&self, id: &str, entries: I) -> Result<()>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let entry = self.inner.get(id).ok_or_else(|| {
            RuuterError::InvalidStep(format!("ws_tag: no such connection '{}'", id))
        })?;
        for (k, v) in entries {
            entry.tags.insert(k, v);
        }
        Ok(())
    }

    /// Snapshot the tags of connection `id`, or `None` if unknown.
    pub fn tags_of(&self, id: &str) -> Option<HashMap<String, String>> {
        self.inner.get(id).map(|c| {
            c.tags
                .iter()
                .map(|t| (t.key().clone(), t.value().clone()))
                .collect()
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_tags_merges_and_broadcast_where_filters() {
        let reg = WsRegistry::new();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), a_tx);
        reg.register("client:b".into(), b_tx);

        reg.set_tags("client:a", [("roles".to_string(), ",admin,ops,".to_string())])
            .unwrap();
        reg.set_tags("client:b", [("roles".to_string(), ",viewer,".to_string())])
            .unwrap();
        // merge, not replace
        reg.set_tags("client:a", [("tenant".to_string(), "acme".to_string())])
            .unwrap();

        let tags = reg.tags_of("client:a").unwrap();
        assert_eq!(tags.get("roles").map(String::as_str), Some(",admin,ops,"));
        assert_eq!(tags.get("tenant").map(String::as_str), Some("acme"));

        let delivered = reg.broadcast_where(
            |_id, t| t.get("roles").is_some_and(|v| v.contains(",admin,")),
            json!({"type": "ping"}),
        );
        assert_eq!(delivered, 1);
        assert!(matches!(a_rx.try_recv(), Ok(Outbound::Json(_))));
        assert!(b_rx.try_recv().is_err());
    }

    #[test]
    fn set_tags_errors_for_unknown_connection() {
        let reg = WsRegistry::new();
        assert!(reg
            .set_tags("client:ghost", [("k".to_string(), "v".to_string())])
            .is_err());
    }

    #[test]
    fn tags_dropped_on_unregister() {
        let reg = WsRegistry::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), tx);
        reg.set_tags("client:a", [("k".to_string(), "v".to_string())])
            .unwrap();
        reg.unregister("client:a");
        assert!(reg.tags_of("client:a").is_none());
    }

    #[test]
    fn broadcast_where_equals_matches_whole_value_only() {
        let reg = WsRegistry::new();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), a_tx);
        reg.register("client:b".into(), b_tx);
        reg.set_tags("client:a", [("tenant".to_string(), "acme".to_string())])
            .unwrap();
        // Substring, not whole match — equals must NOT hit this one.
        reg.set_tags("client:b", [("tenant".to_string(), "acme-eu".to_string())])
            .unwrap();

        let delivered = reg.broadcast_where(
            |_id, t| t.get("tenant").is_some_and(|v| v == "acme"),
            json!({"type": "ping"}),
        );
        assert_eq!(delivered, 1);
        assert!(matches!(a_rx.try_recv(), Ok(Outbound::Json(_))));
        assert!(b_rx.try_recv().is_err());
    }

    #[test]
    fn broadcast_where_skips_connections_missing_the_tag() {
        // Three connections: one has matching tag, one has a
        // different value, one has no such tag key at all. Only the
        // matching one receives.
        let reg = WsRegistry::new();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel();
        let (c_tx, mut c_rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), a_tx);
        reg.register("client:b".into(), b_tx);
        reg.register("client:c".into(), c_tx);
        reg.set_tags("client:a", [("roles".to_string(), ",admin,".to_string())])
            .unwrap();
        reg.set_tags("client:b", [("roles".to_string(), ",viewer,".to_string())])
            .unwrap();
        // client:c stays untagged.

        let delivered = reg.broadcast_where(
            |_id, t| t.get("roles").is_some_and(|v| v.contains(",admin,")),
            json!({}),
        );
        assert_eq!(delivered, 1);
        assert!(matches!(a_rx.try_recv(), Ok(Outbound::Json(_))));
        assert!(b_rx.try_recv().is_err());
        assert!(c_rx.try_recv().is_err());
    }

    #[test]
    fn broadcast_where_returns_zero_when_no_connections_match() {
        let reg = WsRegistry::new();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), a_tx);
        reg.set_tags("client:a", [("roles".to_string(), ",viewer,".to_string())])
            .unwrap();

        let delivered = reg.broadcast_where(
            |_id, t| t.get("roles").is_some_and(|v| v.contains(",admin,")),
            json!({}),
        );
        assert_eq!(delivered, 0);
        assert!(a_rx.try_recv().is_err());
    }

    #[test]
    fn set_tags_second_call_overwrites_same_key() {
        let reg = WsRegistry::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        reg.register("client:a".into(), tx);
        reg.set_tags("client:a", [("roles".to_string(), "viewer".to_string())])
            .unwrap();
        reg.set_tags("client:a", [("roles".to_string(), "admin".to_string())])
            .unwrap();
        assert_eq!(
            reg.tags_of("client:a").unwrap().get("roles").map(String::as_str),
            Some("admin")
        );
    }
}

//! In-process Idempotency-Key dedup cache. Implements PATTERNS.md §2.
//!
//! Keys are `sha256(idempotency_key || method || project || path)`. The
//! HTTP layer computes the key, looks it up, and either returns the
//! cached response (with `Idempotency-Replayed: true`) or runs the DSL
//! and stores the response under this key for future retries.
//!
//! Storage is a `DashMap<[u8; 32], StoredResponse>` with expires-at
//! timestamps. Expired entries are evicted lazily on write — a
//! stale-tolerant approach that avoids a background sweeper thread. For
//! multi-instance deployments this must be swapped for Redis or a
//! Postgres table with `INSERT ... ON CONFLICT DO NOTHING`; that's a
//! framework-level upgrade, no DSL churn.

use dashmap::DashMap;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct StoredResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub headers: HashMap<String, String>,
    expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct IdempotencyStore {
    inner: Arc<DashMap<[u8; 32], StoredResponse>>,
    ttl: Duration,
}

impl IdempotencyStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn dedup_key(
        idempotency_key: &str,
        method: &str,
        project: &str,
        path: &str,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(idempotency_key.as_bytes());
        hasher.update(b"|");
        hasher.update(method.as_bytes());
        hasher.update(b"|");
        hasher.update(project.as_bytes());
        hasher.update(b"|");
        hasher.update(path.as_bytes());
        hasher.finalize().into()
    }

    pub fn get(&self, key: &[u8; 32]) -> Option<StoredResponse> {
        let entry = self.inner.get(key)?;
        if entry.expires_at <= Instant::now() {
            drop(entry);
            self.inner.remove(key);
            return None;
        }
        Some(entry.clone())
    }

    pub fn insert(&self, key: [u8; 32], status: u16, body: Option<Value>, headers: HashMap<String, String>) {
        // Lazy sweep: on every insert, remove ~a handful of expired entries.
        // Bounded per-call cost, unbounded over the lifetime of the process.
        let now = Instant::now();
        let mut swept = 0;
        self.inner.retain(|_, v| {
            if v.expires_at <= now && swept < 32 {
                swept += 1;
                false
            } else {
                true
            }
        });
        self.inner.insert(
            key,
            StoredResponse {
                status,
                body,
                headers,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_key_is_deterministic() {
        let a = IdempotencyStore::dedup_key("k", "POST", "svc", "orders");
        let b = IdempotencyStore::dedup_key("k", "POST", "svc", "orders");
        assert_eq!(a, b);
        let c = IdempotencyStore::dedup_key("k", "POST", "svc", "OTHER");
        assert_ne!(a, c);
    }

    #[test]
    fn expired_entry_is_evicted_on_read() {
        let store = IdempotencyStore::new(Duration::from_millis(10));
        let key = IdempotencyStore::dedup_key("k", "POST", "s", "p");
        store.insert(key, 200, None, HashMap::new());
        assert!(store.get(&key).is_some());
        std::thread::sleep(Duration::from_millis(20));
        assert!(store.get(&key).is_none());
    }
}

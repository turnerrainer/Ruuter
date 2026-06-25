//! Process-wide, project-scoped key/value store.
//!
//! DSLs read and write opaque `serde_json::Value`s. Ruuter has no
//! notion of what the values mean — counters, positions, rolling
//! windows, dedup markers, cached responses, all live under the same
//! primitive. Keys are namespaced by project (the first path segment
//! of the inbound request, or the source's configured project name);
//! a DSL in project `A` cannot read or write keys belonging to
//! project `B`.

use dashmap::DashMap;
use serde_json::Value;
use std::sync::Arc;

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct StateKey {
    pub project: String,
    pub key: String,
}

impl StateKey {
    pub fn new(project: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            key: key.into(),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct StateStore {
    inner: Arc<DashMap<StateKey, Value>>,
}

impl StateStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn get(&self, project: &str, key: &str) -> Option<Value> {
        self.inner
            .get(&StateKey::new(project, key))
            .map(|v| v.clone())
    }

    pub fn set(&self, project: &str, key: &str, value: Value) {
        self.inner.insert(StateKey::new(project, key), value);
    }

    pub fn delete(&self, project: &str, key: &str) -> Option<Value> {
        self.inner.remove(&StateKey::new(project, key)).map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Atomic read-modify-write. Closure receives the current value
    /// (or `None` if absent) and returns the new value.
    pub fn update<F>(&self, project: &str, key: &str, f: F) -> Value
    where
        F: FnOnce(Option<&Value>) -> Value,
    {
        let key = StateKey::new(project, key);
        let mut entry = self.inner.entry(key).or_insert(Value::Null);
        let next = f(Some(&entry));
        *entry = next.clone();
        next
    }
}

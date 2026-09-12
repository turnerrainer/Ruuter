//! Process-wide, project-scoped key/value store.
//!
//! DSLs read and write opaque `serde_json::Value`s. Ruuter has no
//! notion of what the values mean — counters, positions, rolling
//! windows, dedup markers, cached responses, all live under the same
//! primitive. Keys are namespaced by project (the first path segment
//! of the inbound request, or the source's configured project name);
//! a DSL in project `A` cannot read or write keys belonging to
//! project `B`.
//!
//! h2ck.me v1 T-5 — the store now supports an OPTIONAL per-project
//! entry cap (`state.max_entries_per_project` in ruuter.yaml, default
//! 100_000). Pre-fix, `DashMap<StateKey, Value>` had no cap, no TTL,
//! no eviction — a DSL that keyed state on request data
//! (`state.set(key = ${incoming.body.foo})`) could OOM the process.
//! Post-fix, `set` returns `Result<()>` and rejects new keys past
//! the cap; the last-safe insert emits a WARN at 80% of the cap so
//! operators see the trend before the wall.

use crate::{Result, RuuterError};
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

/// Snapshot of a project's state-store footprint, exposed via
/// `StateStore::project_stats` for the optional admin endpoint (T-5
/// leaves the endpoint out — operators diagnose via WARN + cap
/// rejection today).
#[derive(Debug, Clone)]
pub struct ProjectStats {
    pub project: String,
    pub entries: usize,
    /// The cap in effect, if any. `None` means unbounded.
    pub cap: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct StateStore {
    inner: Arc<DashMap<StateKey, Value>>,
    /// h2ck.me v1 T-5 — per-project entry cap. `None` = unbounded
    /// (pre-fix behaviour; safe only when every DSL keys on a
    /// bounded namespace). Set from `state.max_entries_per_project`
    /// in ruuter.yaml.
    max_entries_per_project: Option<usize>,
    /// Per-project entry counts. Read on every `set` for the cap
    /// check; incremented on new-key insert; decremented on
    /// `delete`. Never reflects `Set` on an existing key (that's an
    /// update, no count change).
    project_counts: Arc<DashMap<String, usize>>,
    /// Projects that have already emitted the "reached 80% of cap"
    /// WARN in this process lifetime. Prevents a WARN per insert
    /// once the trend is visible. Only used when a cap is set.
    warned_projects: Arc<DashMap<String, ()>>,
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StateStore {
    /// h2ck.me v1 T-5 — pre-existing API kept unbounded so tests
    /// that never wired a cap continue to work. Production paths
    /// (main.rs, testkit) go through `with_config` instead.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            max_entries_per_project: None,
            project_counts: Arc::new(DashMap::new()),
            warned_projects: Arc::new(DashMap::new()),
        }
    }

    /// h2ck.me v1 T-5 — build a store from `AppConfig::state`. Wires
    /// the operator-configured `max_entries_per_project`; `None`
    /// preserves the pre-fix unbounded behaviour for operators who
    /// opt out explicitly (`state.max_entries_per_project: null`).
    pub fn with_config(cfg: &crate::config::StateConfig) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            max_entries_per_project: cfg.max_entries_per_project,
            project_counts: Arc::new(DashMap::new()),
            warned_projects: Arc::new(DashMap::new()),
        }
    }

    /// Test-only helper: build a store with an explicit cap. Used by
    /// the T-5 regression suite and any future call site that wants
    /// to exercise the cap without going through a full AppConfig.
    pub fn with_max_entries_per_project(max: usize) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            max_entries_per_project: Some(max),
            project_counts: Arc::new(DashMap::new()),
            warned_projects: Arc::new(DashMap::new()),
        }
    }

    pub fn get(&self, project: &str, key: &str) -> Option<Value> {
        self.inner
            .get(&StateKey::new(project, key))
            .map(|v| v.clone())
    }

    /// h2ck.me v1 T-5 — `set` now returns `Result<()>` so an
    /// oversubscribed project fails cleanly with an
    /// `InvalidStep`-flavoured error instead of silently growing
    /// the map. Pre-T-5 callers that ignored the return value need
    /// to `?` it or handle explicitly. Setting an EXISTING key is
    /// always allowed (it's an update, no count change).
    pub fn set(&self, project: &str, key: &str, value: Value) -> Result<()> {
        let state_key = StateKey::new(project, key);
        // Update path — key already present, no count change.
        if self.inner.contains_key(&state_key) {
            self.inner.insert(state_key, value);
            return Ok(());
        }
        // New-key path — cap check + count bump.
        if let Some(cap) = self.max_entries_per_project {
            let current = self.project_counts.get(project).map(|v| *v).unwrap_or(0);
            if current >= cap {
                return Err(RuuterError::InvalidStep(format!(
                    "state.set rejected for project '{}': entry count {} \
                     reached the cap max_entries_per_project={} (h2ck.me v1 T-5). \
                     Delete unused keys or raise the cap in ruuter.yaml.",
                    project, current, cap
                )));
            }
            // 80% WARN once per project per process lifetime. Using
            // `saturating_mul` so a cap near usize::MAX doesn't panic;
            // integer division floors, which is fine — the point is
            // "you're approaching the wall," not an exact percentage.
            let warn_threshold = cap.saturating_mul(80) / 100;
            if current + 1 == warn_threshold && !self.warned_projects.contains_key(project) {
                self.warned_projects.insert(project.to_string(), ());
                tracing::warn!(
                    project = %project,
                    entries = current + 1,
                    cap = cap,
                    "state store for project reached 80% of max_entries_per_project cap \
                     (h2ck.me v1 T-5) — inserts will start failing at {}%",
                    100
                );
            }
            self.project_counts
                .entry(project.to_string())
                .and_modify(|c| *c += 1)
                .or_insert(1);
        }
        self.inner.insert(state_key, value);
        Ok(())
    }

    pub fn delete(&self, project: &str, key: &str) -> Option<Value> {
        let removed = self
            .inner
            .remove(&StateKey::new(project, key))
            .map(|(_, v)| v);
        // Decrement project count only on real removal AND only when
        // a cap is in effect (no need to track counts otherwise).
        if removed.is_some() && self.max_entries_per_project.is_some() {
            self.project_counts
                .entry(project.to_string())
                .and_modify(|c| *c = c.saturating_sub(1));
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// h2ck.me v1 T-5 — per-project entry count. Returns 0 for a
    /// project with no entries. Used by the boot-time WARN scan and
    /// the `project_stats` iterator; also handy for tests.
    pub fn project_entry_count(&self, project: &str) -> usize {
        // If no cap is set, fall back to scanning the map — the
        // project_counts DashMap is only maintained when a cap is
        // active. Scan cost is only paid by callers that ask, and
        // isn't on the hot path.
        if self.max_entries_per_project.is_some() {
            self.project_counts.get(project).map(|v| *v).unwrap_or(0)
        } else {
            self.inner
                .iter()
                .filter(|entry| entry.key().project == project)
                .count()
        }
    }

    /// h2ck.me v1 T-5 — the cap in effect (or None for unbounded).
    pub fn max_entries_per_project(&self) -> Option<usize> {
        self.max_entries_per_project
    }

    /// h2ck.me v1 T-5 — test accessor that returns whether the
    /// store has already emitted the once-per-project 80% WARN
    /// for `project`. Exposed so regression tests can assert on
    /// the deterministic internal state without racing against
    /// the (thread-local) tracing subscriber capture. Not part of
    /// the operator-facing API surface.
    pub fn warned_projects_contains(&self, project: &str) -> bool {
        self.warned_projects.contains_key(project)
    }

    /// h2ck.me v1 T-5 — every project that has state in the store.
    /// Snapshot; concurrent modification isn't reflected.
    pub fn project_stats(&self) -> Vec<ProjectStats> {
        let cap = self.max_entries_per_project;
        if cap.is_some() {
            self.project_counts
                .iter()
                .map(|entry| ProjectStats {
                    project: entry.key().clone(),
                    entries: *entry.value(),
                    cap,
                })
                .collect()
        } else {
            // Unbounded — scan the map and group. Only invoked by
            // operator-diagnostic callers, so the cost is fine.
            let mut counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for entry in self.inner.iter() {
                *counts.entry(entry.key().project.clone()).or_insert(0) += 1;
            }
            counts
                .into_iter()
                .map(|(project, entries)| ProjectStats {
                    project,
                    entries,
                    cap: None,
                })
                .collect()
        }
    }

    /// Atomic read-modify-write. Closure receives the current value
    /// (or `None` if absent) and returns the new value.
    ///
    /// h2ck.me v1 T-5 — `update` on a new-key insert path also
    /// honours the cap. If the key is absent and the cap is reached,
    /// this returns `Err`; existing-key update paths are always
    /// allowed. Signature is `Result<Value>` (was `Value` pre-fix)
    /// so cap breaches surface cleanly.
    pub fn update<F>(&self, project: &str, key: &str, f: F) -> Result<Value>
    where
        F: FnOnce(Option<&Value>) -> Value,
    {
        let state_key = StateKey::new(project, key);
        // The DashMap `.entry` API doesn't let us "peek then decide"
        // cheaply against a per-project cap, so split the read and
        // write. Race: two concurrent updates on the same new key
        // may both pass the cap check and both insert; the count
        // then briefly reports `current + 2` before settling — this
        // is a soft cap, not a hard one, and 1-off overshoot on
        // contention is preferable to holding a lock across the
        // closure. Callers should not rely on the cap for correctness.
        let present = self.inner.contains_key(&state_key);
        if !present {
            if let Some(cap) = self.max_entries_per_project {
                let current = self.project_counts.get(project).map(|v| *v).unwrap_or(0);
                if current >= cap {
                    return Err(RuuterError::InvalidStep(format!(
                        "state.update rejected for project '{}': entry count {} \
                         reached the cap max_entries_per_project={} (h2ck.me v1 T-5).",
                        project, current, cap
                    )));
                }
            }
        }
        let mut entry = self.inner.entry(state_key).or_insert(Value::Null);
        let next = f(Some(&entry));
        *entry = next.clone();
        if !present && self.max_entries_per_project.is_some() {
            self.project_counts
                .entry(project.to_string())
                .and_modify(|c| *c += 1)
                .or_insert(1);
        }
        Ok(next)
    }
}

//! h2ck.me v1 T-24 — `StateStore::set` TOCTOU on `contains_key` +
//! `insert`.
//!
//! Pre-fix (v0.10.0-rc / T-5): the update-vs-new-key branch in
//! `StateStore::set` split `contains_key(&key)` from
//! `inner.insert(key, value)`. Two threads racing on the SAME new
//! key could both observe "absent" and both walk the new-key path,
//! each bumping the per-project counter, before either performed
//! the actual insert. The map's `insert` is idempotent, so the
//! stored state was correct — the per-project counter over-reported
//! by (N contenders - 1). With T-5's per-project entry cap, that
//! over-count triggers premature "cap reached" rejections under
//! contention. No auth or data-corruption impact; stability /
//! fairness bug only.
//!
//! Post-fix (T-24): the branch is decided under the DashMap shard
//! lock via the `entry` API. The `Vacant` arm sees "new key"
//! exactly once per real insert, so the counter is bumped at most
//! once per real insert. Same-key contention settles at the
//! truthful count.
//!
//! Tests written to try to BREAK the fix:
//! - 200 threads racing on the SAME new key under
//!   `max_entries_per_project=1` — the store must not reject
//!   (idempotent update), the counter must read 1, the stored
//!   value must be one of the contenders' values.
//! - 200 threads racing on the SAME new key with NO cap — the
//!   counter is untracked, but a subsequent single-thread
//!   `project_entry_count` scan must report exactly 1 stored key.
//! - 200 threads racing on DIFFERENT new keys under
//!   `max_entries_per_project=200` — every set must succeed,
//!   counter reads exactly 200.
//! - 200 threads racing on DIFFERENT new keys under
//!   `max_entries_per_project=50` — the first ~50 succeed (some
//!   ordering-dependent), the rest fail cleanly with the cap
//!   error, and the counter reads exactly 50 at rest.
//! - Serial re-set of the same key past cap — must still succeed
//!   as an update (no cap change), covers the update path of the
//!   fixed branch.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::state::StateStore;
use serde_json::json;
use std::sync::Arc;
use std::thread;

/// The regression pin named in the backlog: 200 concurrent tasks
/// racing on the SAME new key under `max_entries_per_project=1`.
/// Pre-fix, the counter drifted to N; post-fix it stays at 1.
#[test]
fn concurrent_same_key_writes_do_not_over_charge_counter() {
    const THREADS: usize = 200;
    let store = Arc::new(StateStore::with_max_entries_per_project(1));

    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || s.set("p", "k", json!(i))));
    }

    let mut ok_count = 0usize;
    for h in handles {
        if h.join().expect("worker panicked").is_ok() {
            ok_count += 1;
        }
    }

    // Every contender is either the first insert (Vacant → new-key
    // path, admits under cap=1) or a subsequent update (Occupied →
    // no count change, admits regardless of cap). Both branches
    // return Ok — so every join returns Ok.
    assert_eq!(
        ok_count, THREADS,
        "same-key contention must never reject: each contender is either the first \
         insert or a subsequent update, both of which are allowed. Pre-fix, some \
         inserts saw stale over-counted values and rejected."
    );

    // The core assertion: counter reads exactly 1, not >1.
    assert_eq!(
        store.project_entry_count("p"),
        1,
        "counter must reflect exactly one stored key after same-key contention; \
         pre-fix, the counter drifted to as many as {} under TOCTOU",
        THREADS
    );

    // And the stored value is one of the contenders' values.
    let v = store.get("p", "k").expect("key must be stored");
    let n = v.as_i64().expect("value is i64");
    assert!(
        (0..THREADS as i64).contains(&n),
        "stored value must be one of the contenders' inputs; got {}",
        n
    );
}

/// Same shape but with NO cap configured. Counter isn't tracked in
/// this mode, so we verify via `project_entry_count`'s fallback
/// (which scans the inner map when cap is None) that exactly one
/// key was actually stored.
#[test]
fn concurrent_same_key_writes_store_exactly_one_key_uncapped() {
    const THREADS: usize = 200;
    let store = Arc::new(StateStore::new());

    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            s.set("p", "same_key", json!(i))
                .expect("uncapped never rejects");
        }));
    }
    for h in handles {
        h.join().expect("worker panicked");
    }

    assert_eq!(
        store.project_entry_count("p"),
        1,
        "exactly one stored key for project p"
    );
    assert_eq!(store.len(), 1, "exactly one entry in the whole store");
}

/// Different-key contention with headroom — every set fits, no
/// rejections, counter reads exactly N.
#[test]
fn concurrent_different_keys_all_admit_under_matching_cap() {
    const THREADS: usize = 200;
    let store = Arc::new(StateStore::with_max_entries_per_project(THREADS));

    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            s.set("p", &format!("k{}", i), json!(i))
                .expect("cap has headroom");
        }));
    }
    for h in handles {
        h.join().expect("worker panicked");
    }

    assert_eq!(store.project_entry_count("p"), THREADS);
    assert_eq!(store.len(), THREADS);
}

/// Different-key contention past cap. Some threads win, some lose;
/// what matters is (a) the counter settles at exactly the cap, (b)
/// rejections carry the T-5 error shape, (c) no thread panics.
///
/// This is where the T-24 fix matters most in the wild: with
/// `contains_key` + `insert`, N contenders on N different new keys
/// could each pass the cap check, then race on the counter bump,
/// briefly over-counting past the cap. Post-fix the shard-lock
/// gate keeps the counter honest.
#[test]
fn concurrent_different_keys_past_cap_settle_at_exactly_cap() {
    const THREADS: usize = 200;
    const CAP: usize = 50;
    let store = Arc::new(StateStore::with_max_entries_per_project(CAP));

    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let s = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            let res = s.set("p", &format!("k{}", i), json!(i));
            (i, res)
        }));
    }

    let mut ok = 0usize;
    let mut errs = 0usize;
    for h in handles {
        let (_i, res) = h.join().expect("worker panicked");
        match res {
            Ok(()) => ok += 1,
            Err(e) => {
                errs += 1;
                let msg = format!("{e}");
                assert!(
                    msg.contains("state.set rejected") && msg.contains("cap"),
                    "reject must carry the T-5 error shape; got: {msg}"
                );
            }
        }
    }

    assert_eq!(ok + errs, THREADS, "every worker either admits or rejects");
    assert_eq!(ok, CAP, "exactly CAP threads succeed (rest reject cleanly)");
    assert_eq!(
        store.project_entry_count("p"),
        CAP,
        "counter must settle at exactly cap after contention; pre-fix, race on \
         `contains_key` + `insert` briefly over-counted"
    );
    assert_eq!(store.len(), CAP);
}

/// Serial re-set of the same key past cap — the update path in the
/// fixed `entry`-based branch must not increment the counter and
/// must still admit. Guards against the Occupied arm accidentally
/// treating a re-set as a new insert.
#[test]
fn serial_reset_of_existing_key_past_cap_still_admits() {
    let store = StateStore::with_max_entries_per_project(1);
    store.set("p", "k", json!(1)).unwrap();
    // At cap. Re-setting the same key is an UPDATE — always allowed.
    for i in 2..10 {
        store
            .set("p", "k", json!(i))
            .expect("re-set of existing key must always admit");
    }
    assert_eq!(store.project_entry_count("p"), 1);
    assert_eq!(store.get("p", "k"), Some(json!(9)));

    // A different NEW key still rejects — cap enforcement intact.
    let err = store.set("p", "other", json!(0)).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("state.set rejected"),
        "new-key insert past cap must still reject; got: {msg}"
    );
}

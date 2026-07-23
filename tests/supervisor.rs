//! Integration tests for the source supervisor (#008).
//!
//! The supervisor doesn't know about WS specifically — it watchdogs
//! any `Fn() -> Future<Output = ()>`. These tests exercise that
//! generic interface so we can prove the restart / status logic
//! without spinning up real network sockets.

use ruuter_on_rust::supervisor::{SourceId, SourceStatus, SourceSupervisor};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn id(name: &str) -> SourceId {
    SourceId {
        project: "test".into(),
        name: name.into(),
        kind: "synthetic".into(),
    }
}

#[tokio::test]
async fn clean_return_marks_source_dead_no_restart() {
    let sup = SourceSupervisor::new();
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();

    sup.supervise(id("clean"), move || {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            // Clean return — supervisor must mark Dead and stop.
        }
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let report = sup.report();
    let state = report
        .sources
        .iter()
        .find(|s| s.id.name == "clean")
        .expect("found");
    assert!(
        matches!(state.status, SourceStatus::Dead { .. }),
        "got {:?}",
        state.status
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "should NOT have restarted"
    );
}

#[tokio::test]
async fn panic_triggers_restart_with_backoff() {
    let sup = SourceSupervisor::new();
    let counter = Arc::new(AtomicU32::new(0));
    let c = counter.clone();

    sup.supervise(id("panicky"), move || {
        let c = c.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                panic!("synthetic panic #{}", n);
            }
            // After 2 panics, return cleanly so the watchdog stops.
        }
    });

    // Give the supervisor enough wall-clock time to:
    //   spawn → panic → backoff 500ms → spawn → panic → backoff 1000ms → spawn → clean → Dead.
    tokio::time::sleep(Duration::from_millis(4_000)).await;

    let report = sup.report();
    let state = report
        .sources
        .iter()
        .find(|s| s.id.name == "panicky")
        .expect("found");
    assert!(
        matches!(state.status, SourceStatus::Dead { .. }),
        "got {:?}",
        state.status
    );
    assert!(
        counter.load(Ordering::SeqCst) >= 3,
        "spawned at least 3 times"
    );
    assert!(
        state.restart_count >= 2,
        "restart_count should reflect panics; got {}",
        state.restart_count
    );
}

#[tokio::test]
async fn report_aggregates_counts_correctly() {
    let sup = SourceSupervisor::new();

    // Source 1: clean exit → Dead.
    sup.supervise(id("a"), || async {});

    // Source 2: returns immediately too, also Dead.
    sup.supervise(id("b"), || async {});

    // Source 3: long-running → Running until we tear it down.
    sup.supervise(id("c"), || async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let r = sup.report();
    assert_eq!(r.total, 3);
    assert_eq!(r.dead, 2, "a and b clean-exited");
    assert!(r.running >= 1, "c still running; got {}", r.running);
}

#[tokio::test]
async fn admin_route_returns_supervisor_report() {
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    let sup = Arc::new(SourceSupervisor::new());
    sup.supervise(id("alpha"), || async {
        // Long-lived so it stays Running while we query.
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let app = sup.clone().admin_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/_/sources")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body_str = std::str::from_utf8(&body).unwrap();
    let json: serde_json::Value = serde_json::from_str(body_str).expect("valid json");

    assert_eq!(json["total"], 1);
    assert_eq!(json["sources"][0]["id"]["name"], "alpha");
    assert_eq!(json["sources"][0]["id"]["kind"], "synthetic");
}

#[tokio::test]
async fn supervised_task_restart_uses_exponential_backoff() {
    // We can't easily test exact backoff timings without flakiness,
    // but we CAN verify that restart_count climbs and the next-attempt
    // budget reported in the Restarting status increases.
    let sup = SourceSupervisor::new();

    sup.supervise(id("always_panics"), move || async {
        panic!("intentional");
    });

    // Sample multiple times — at least one sample must catch the
    // source in Restarting (between panic and respawn).
    let mut max_observed_backoff: u64 = 0;
    let mut max_restart_count: u32 = 0;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let r = sup.report();
        if let Some(s) = r.sources.iter().find(|s| s.id.name == "always_panics") {
            if let SourceStatus::Restarting {
                restart_count,
                next_attempt_in_ms,
                ..
            } = &s.status
            {
                max_observed_backoff = max_observed_backoff.max(*next_attempt_in_ms);
                max_restart_count = max_restart_count.max(*restart_count);
            }
        }
    }

    assert!(
        max_restart_count >= 2,
        "should have observed multiple restarts; got {}",
        max_restart_count
    );
    assert!(
        max_observed_backoff >= 1_000,
        "backoff should grow past initial 500ms; observed max {}ms",
        max_observed_backoff
    );
}

//! Task 047 spike — answer the ONE question: is `rquickjs::Context`
//! (and `AsyncContext`) `Send + Sync`?
//!
//! **FINDING (2026-07-19, rquickjs 0.6.2):**
//!
//! - Default features: `Runtime` and `Context` are `!Send` (embed
//!   `Rc<Mut<RawRuntime>>` and `NonNull<JSContext>` respectively).
//!   Same architectural constraint as Boa. No escape.
//! - **With features `["parallel", "futures"]`: `Runtime`,
//!   `Context`, `AsyncRuntime`, `AsyncContext` are ALL `Send + Sync`.**
//!   Verified compile-time via `assert_send<T>()` markers AND
//!   runtime by holding an AsyncContext across `.await` on a
//!   multi-thread tokio runtime AND spawning it into another task.
//!
//! **Consequence:** the compound-win path for tasks 036 (per-request
//! BoaContext pool) and 045 (pre-parsed Script cache) IS open.
//! Neither task can be built on Boa without a dedicated OS worker
//! thread pool, but both are straightforward on rquickjs (parallel
//! feature) because Send/Sync just works.
//!
//! **Not answered by this spike:**
//! - DSL-level scenario compatibility. rquickjs is a QuickJS
//!   wrapper; QuickJS is highly ECMAScript-compatible but not
//!   byte-identical to Boa on edge cases (Date parsing, regex,
//!   Number precision).
//! - Perf on the actual Ruuter DSL corpus. rquickjs published
//!   benchmarks show 2-5× vs Boa on typical workloads, but our
//!   corpus specifics need measurement.
//! - Binary size delta (adds ~500 KB for rquickjs + rquickjs-sys).
//! - CVE surface (QuickJS is a C library — same broad category
//!   as libssl, smaller than V8 by orders of magnitude but non-zero).
//!
//! The full engine-swap follow-up is filed as task 051.
//!
//! Build: cargo test --release --features spike-quickjs \
//!          --test spike_047_quickjs_send
//!
//! Without the feature this file is a no-op (empty test module) so
//! the default build isn't affected.

#![cfg(feature = "spike-quickjs")]

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn assert_send_sync<T: Send + Sync>() {}

// ── Sync core types ────────────────────────────────────────────

#[test]
fn runtime_is_send() {
    // Runtime holds the JS heap. If it's !Send, all downstream
    // types will be too — check root before descending.
    assert_send::<rquickjs::Runtime>();
}

#[test]
fn context_is_send() {
    // The crucial one. If Context is Send, task 036's "hold
    // engine in ExecutionContext across .await" becomes trivial.
    assert_send::<rquickjs::Context>();
}

// ── Async wrappers ────────────────────────────────────────────

#[test]
fn async_runtime_is_send_sync() {
    assert_send_sync::<rquickjs::AsyncRuntime>();
}

#[test]
fn async_context_is_send_sync() {
    // rquickjs's `AsyncContext` is designed for use with async
    // runtimes; if THIS is Send + Sync, we can share it across
    // tokio worker threads without ceremony.
    assert_send_sync::<rquickjs::AsyncContext>();
}

// ── Can we hold one across .await? ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_context_survives_await_boundary() {
    // The concrete test task 036 needs: build a context, hold it
    // in a struct, use it across .await points on a multi-thread
    // runtime. If this compiles and passes, the unblock path is
    // clear.
    let rt = rquickjs::AsyncRuntime::new().expect("runtime");
    let ctx = rquickjs::AsyncContext::full(&rt).await.expect("context");

    // Yield to force the multi-thread runtime to potentially
    // migrate this task between workers.
    tokio::task::yield_now().await;

    // Evaluate a trivial expression.
    let result: i32 = ctx
        .with(|ctx| ctx.eval::<i32, _>("1 + 2"))
        .await
        .expect("eval");
    assert_eq!(result, 3);

    // Second use, another await between.
    tokio::task::yield_now().await;
    let result2: String = ctx
        .with(|ctx| ctx.eval::<String, _>("'hello ' + 'world'"))
        .await
        .expect("eval2");
    assert_eq!(result2, "hello world");
}

// ── Spawn the context across tokio tasks ───────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_context_used_from_spawned_task() {
    // Explicit: create context on one task, use it on another via
    // tokio::spawn. If this compiles, task 036 can hand a shared
    // Arc<AsyncContext> around freely.
    let rt = rquickjs::AsyncRuntime::new().expect("runtime");
    let ctx = std::sync::Arc::new(rquickjs::AsyncContext::full(&rt).await.expect("context"));

    let ctx1 = ctx.clone();
    let h1 = tokio::spawn(async move { ctx1.with(|ctx| ctx.eval::<i32, _>("10 * 10")).await });
    let ctx2 = ctx.clone();
    let h2 = tokio::spawn(async move { ctx2.with(|ctx| ctx.eval::<i32, _>("20 * 20")).await });

    assert_eq!(h1.await.unwrap().unwrap(), 100);
    assert_eq!(h2.await.unwrap().unwrap(), 400);
}

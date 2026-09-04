//! Issue #37 — Java-parity per-step INFO log lines.
//!
//! These tests boot a tiny in-memory DSL, run it through the router,
//! and assert the tracing output contains the "Executed" INFO line
//! for every step plus the DSL-run bracket lines. They also verify
//! step-type-specific `attrs` (HTTP URL + upstream status, switch
//! matched branch, return status, state op/key, log message).
//!
//! Regression watch: this is the trail Java Ruuter's
//! `LoggingUtils.logStep()` emitted at INFO on every step. Losing
//! any of it turns DSL execution back into a black box for
//! operators, which is exactly what issue #37 flagged.

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// `MakeWriter` that appends every write into a shared `Vec<u8>` so
/// the test can inspect the captured tracing output. Cheap enough
/// for one-shot per-test capture; each test builds its own instance
/// so cross-test bleed via a shared subscriber is impossible.
#[derive(Clone)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn build_router(project: &str, method: &str, path: &str, body: &str) -> DslRouter {
    build_router_with_cfg(project, method, path, body, |_| {})
}

/// Same as `build_router` but with a config mutator so tests can flip
/// `log_step_executions` / `log_dsl_runs` before the engine picks up
/// the logging block.
fn build_router_with_cfg(
    project: &str,
    method: &str,
    path: &str,
    body: &str,
    tweak: impl FnOnce(&mut AppConfig),
) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-log-test-{}", uniq()));
    let dir = tmp.join(project).join(method);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{}.yml", path)), body).unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp;
    tweak(&mut cfg);

    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let dsls = loader.load_all().expect("load dsls");
    let ws_registry = WsRegistry::new();
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_logging(std::sync::Arc::new(cfg.logging.clone()))
        .with_ws_registry(ws_registry.clone());
    DslRouter::new(
        dsls,
        HashMap::new(),
        cfg,
        StateStore::new(),
        ws_registry,
        engine,
    )
}

fn uniq() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Install `buf` as the tracing sink for the current thread and
/// return a scoped guard. Callers keep the guard alive for the
/// duration of the test body — dropping it restores the previous
/// subscriber so parallel test threads don't inherit ours.
fn capture(buf: SharedBuf) -> tracing::subscriber::DefaultGuard {
    use tracing_subscriber::{fmt, EnvFilter};
    let subscriber = fmt()
        .with_writer(buf)
        .with_max_level(tracing::Level::INFO)
        .with_env_filter(EnvFilter::new("info"))
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::set_default(subscriber)
}

#[tokio::test(flavor = "current_thread")]
async fn assign_return_dsl_emits_executed_lines_and_bracket() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
prep:
  assign:
    who: "world"
  next: reply

reply:
  return: { hello: "${who}" }
  next: end
"#;
    // `log_dsl_runs` is OFF by default — the request span already
    // brackets each run via trace_id and the access log covers the
    // response summary. Test explicitly turns it on to verify both
    // shapes (per-step trail + brackets) work end-to-end.
    let router = build_router_with_cfg("svc", "GET", "hello", dsl, |cfg| {
        cfg.logging.log_dsl_runs = true;
    });
    let res = router
        .execute_dsl(
            "svc",
            "GET",
            "hello",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["hello"], "world");

    let out = buf.contents();
    let exec_lines = out.matches("Executed").count();
    assert!(
        exec_lines >= 2,
        "expected ≥2 Executed lines (one per DSL step), got {} in: {}",
        exec_lines,
        out
    );
    assert!(
        out.contains("DSL run started"),
        "expected 'DSL run started' bracket in: {}",
        out
    );
    assert!(
        out.contains("DSL run completed"),
        "expected 'DSL run completed' bracket in: {}",
        out
    );
    assert!(
        out.contains("keys"),
        "assign step should surface keys in attrs: {}",
        out
    );
    assert!(
        out.contains("status=200"),
        "return step should surface status=200 in attrs: {}",
        out
    );
    assert!(
        out.contains("terminated_by=\"return\"") || out.contains("terminated_by=return"),
        "DSL-run-completed line should carry terminated_by=return: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn switch_step_reports_matched_branch_in_attrs() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
route:
  switch:
    - condition: "${incoming.body.mode == 'a'}"
      next: reply_a
    - condition: "${incoming.body.mode == 'b'}"
      next: reply_b
  next: reply_default

reply_a:
  return: { picked: "a" }
  next: end

reply_b:
  return: { picked: "b" }
  next: end

reply_default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "POST", "pick", dsl);
    let mut body = HashMap::new();
    body.insert("mode".to_string(), serde_json::json!("b"));
    let res = router
        .execute_dsl(
            "svc",
            "POST",
            "pick",
            body,
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["picked"], "b");

    let out = buf.contents();
    assert!(
        out.contains("condition=1"),
        "expected condition=1 for the second slot in the switch list: {}",
        out
    );
    // switch.next was dropped from the attrs field (redundant with
    // the engine's own "→ next-step" positional column). The routed
    // Executed line for reply_b confirms the switch went there.
    assert!(
        out.contains("dsl.step=reply_b"),
        "expected an Executed line for reply_b: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn switch_no_match_reports_condition_no_match() {
    // Regression: the no-match case must use the same `condition=`
    // field name as the match case so a single grep predicate
    // catches both branches. Value is `no_match` (unquoted,
    // snake_case). Two earlier iterations existed: `matched="no-match"`
    // (Java-parity name — inconsistent field name) and
    // `condition=undefined` (#37, JS-native sentinel — opaque to
    // readers who don't know JS); the current shape settled after
    // #54.
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
route:
  switch:
    - condition: "${incoming.body.mode == 'a'}"
      next: reply_a
  next: reply_default

reply_a:
  return: { picked: "a" }
  next: end

reply_default:
  return: { picked: "default" }
  next: end
"#;
    let router = build_router("svc", "POST", "no-match", dsl);
    let mut body = HashMap::new();
    body.insert("mode".to_string(), serde_json::json!("z")); // won't match
    router
        .execute_dsl(
            "svc",
            "POST",
            "no-match",
            body,
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");

    let out = buf.contents();
    assert!(
        out.contains("condition=no_match"),
        "expected condition=no_match for no-match case: {}",
        out
    );
    // Guard against every prior shape of this field slipping back:
    //   - `matched="no-match"` (pre-#37, wrong field name)
    //   - `condition=undefined`  (post-#37, JS-native but opaque)
    //   - `condition="no_match"` (quoted — Display path, not `push_preformatted`)
    //   - `branch=…`             (older Rust-only attr name)
    assert!(
        !out.contains("matched=\"no-match\"")
            && !out.contains("condition=undefined")
            && !out.contains("condition=\"no_match\"")
            && !out.contains("branch="),
        "old switch attr shapes must not appear: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn state_step_reports_op_and_key_in_attrs() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
put:
  state:
    set: { key: "session", value: "abc" }
  next: fetch

fetch:
  state:
    get: { key: "session", into: current }
  next: reply

reply:
  return: { value: "${current}" }
  next: end
"#;
    let router = build_router("svc", "POST", "session", dsl);
    let res = router
        .execute_dsl(
            "svc",
            "POST",
            "session",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");
    assert_eq!(res.value.unwrap()["value"], "abc");

    let out = buf.contents();
    assert!(
        out.contains("op=\"set\"") && out.contains("key=\"session\""),
        "set line should surface op+key: {}",
        out
    );
    assert!(
        out.contains("op=\"get\"") && out.contains("hit=true"),
        "get line should surface op+hit: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disabling_log_step_executions_silences_per_step_lines_only() {
    // Verifies the two knobs are independent — turning off the
    // per-step trail must not accidentally silence the DSL-run
    // brackets, and turning off the brackets must not silence the
    // per-step trail. Regression watch for a future refactor that
    // collapses both under one gate.
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
prep:
  assign: { who: "world" }
  next: reply

reply:
  return: { hello: "${who}" }
  next: end
"#;
    let router = build_router_with_cfg("svc", "GET", "quiet", dsl, |cfg| {
        cfg.logging.log_step_executions = false;
        cfg.logging.log_dsl_runs = true;
    });
    router
        .execute_dsl(
            "svc",
            "GET",
            "quiet",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");

    let out = buf.contents();
    assert!(
        !out.contains("Executed"),
        "log_step_executions=false must silence per-step lines: {}",
        out
    );
    assert!(
        out.contains("DSL run started") && out.contains("DSL run completed"),
        "brackets stay on independently: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disabling_log_dsl_runs_silences_brackets_only() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
prep:
  assign: { who: "world" }
  next: reply

reply:
  return: { hello: "${who}" }
  next: end
"#;
    let router = build_router_with_cfg("svc", "GET", "no-bracket", dsl, |cfg| {
        cfg.logging.log_step_executions = true;
        cfg.logging.log_dsl_runs = false; // explicit no-brackets
    });
    router
        .execute_dsl(
            "svc",
            "GET",
            "no-bracket",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");

    let out = buf.contents();
    assert!(
        !out.contains("DSL run started") && !out.contains("DSL run completed"),
        "log_dsl_runs=false must silence brackets: {}",
        out
    );
    assert!(
        out.contains("Executed"),
        "per-step trail stays on independently: {}",
        out
    );
}

#[tokio::test(flavor = "current_thread")]
async fn log_step_message_appears_in_attrs() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
say:
  log: "beacon-abc123"
  next: reply

reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "log-test", dsl);
    router
        .execute_dsl(
            "svc",
            "GET",
            "log-test",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");

    let out = buf.contents();
    assert!(
        out.contains("msg=\"beacon-abc123\""),
        "log step should surface evaluated message in attrs: {}",
        out
    );
}

/// Issue #56 — `log:` accepts a mapping. Every string leaf runs
/// through the script engine (same as `assign:`) so `${…}` works
/// under a nested map; the evaluated shape renders as compact JSON
/// in the `attrs.msg` field.
#[tokio::test(flavor = "current_thread")]
async fn log_step_accepts_map_form() {
    let buf = SharedBuf::new();
    let _guard = capture(buf.clone());
    let dsl = r#"
prep:
  assign:
    who: "alice"
  next: say

say:
  log:
    user: "${who}"
    action: "login"
    count: 3
  next: reply

reply:
  return: { ok: true }
  next: end
"#;
    let router = build_router("svc", "GET", "log-map", dsl);
    router
        .execute_dsl(
            "svc",
            "GET",
            "log-map",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "test".into(),
        )
        .await
        .expect("exec");

    let out = buf.contents();
    // Interpolated leaf value, literal leaf, and numeric leaf must
    // all round-trip into the rendered JSON payload on `attrs.msg`.
    assert!(
        out.contains(r#""user":"alice""#),
        "map-form log must interpolate string leaves: {}",
        out
    );
    assert!(
        out.contains(r#""action":"login""#),
        "map-form log must preserve literal string leaves: {}",
        out
    );
    assert!(
        out.contains(r#""count":3"#),
        "map-form log must preserve numeric leaves: {}",
        out
    );
}

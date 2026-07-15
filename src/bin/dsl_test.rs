//! dsl-test — comprehensive test runner for the Ruuter DSL tree.
//!
//! Walks `--tests` (default `./DSL-tests`), collects every `*.test.yml`
//! file, runs its scenarios against the DSL tree at `--dsl`, and
//! reports per-scenario pass/fail. Exit non-zero on any failure.
//!
//! Modes (declared in each test file via `mode:`):
//!
//! - `inprocess` (default) — HTTP request through `DslRouter::execute_dsl`.
//! - `mock-http` — same as inprocess but boots an in-process upstream
//!   mock; `setup.mocks` and `verify_mocks` become available.
//! - `ws-client` — binds an axum test server, opens a real WS client,
//!   sends `send:` frames, waits for `expect_frames:`.
//! - `trigger-inject` — synthetic frame through `TriggerDispatcher`.
//!
//! Usage:
//!   dsl-test
//!   dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
//!   dsl-test --filter GET/basic       # substring on test-file path
//!   dsl-test --json                   # machine-readable summary

use futures::{SinkExt, StreamExt};
use ruuter_on_rust::config::{load_constants, AppConfig};
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::testkit::harness::{Harness, HarnessResponse};
use ruuter_on_rust::testkit::matcher::{deep_equal, subset_matches};
use ruuter_on_rust::testkit::mock_http::MockServer;
use ruuter_on_rust::testkit::schema::{ExpectHttp, Mode, Scenario, TestFile};
use ruuter_on_rust::triggers::TriggerDispatcher;
use ruuter_on_rust::ws::WsRegistry;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let base_constants = load_constants(args.constants.to_str().unwrap_or("./constants.ini"))
        .unwrap_or_default();

    let test_files = enumerate_test_files(&args.tests_root);
    if test_files.is_empty() {
        eprintln!(
            "dsl-test: no *.test.yml files found under {}",
            args.tests_root.display()
        );
        return ExitCode::from(1);
    }

    let mut summary = RunSummary::new(args.json);

    for test_file_path in &test_files {
        if let Some(f) = &args.filter {
            if !test_file_path.to_string_lossy().contains(f) {
                continue;
            }
        }

        let file: TestFile = match load_test_file(test_file_path) {
            Ok(f) => f,
            Err(e) => {
                summary.record(
                    test_file_path,
                    "<parse>",
                    false,
                    Some(format!("failed to load test file: {}", e)),
                );
                continue;
            }
        };

        // Merge constants: per-file overrides on top of base constants.
        let mut constants = base_constants.clone();
        constants.extend(file.constants.clone());

        // Build one harness per test file. This makes constants + state
        // per-file, which matches the semantics of `mode:` — each file
        // is a hermetic run.
        match run_file(&args, &file, test_file_path, constants).await {
            Ok(scenario_results) => {
                for (name, ok, err) in scenario_results {
                    summary.record(test_file_path, &name, ok, err);
                }
            }
            Err(e) => {
                summary.record(
                    test_file_path,
                    "<harness>",
                    false,
                    Some(format!("harness failure: {}", e)),
                );
            }
        }
    }

    summary.emit();
    if summary.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

struct Args {
    dsl_root: PathBuf,
    tests_root: PathBuf,
    constants: PathBuf,
    filter: Option<String>,
    json: bool,
}

impl Args {
    fn parse() -> Self {
        let mut dsl_root = PathBuf::from("./DSL");
        let mut tests_root = PathBuf::from("./DSL-tests");
        let mut constants = PathBuf::from("./constants.ini");
        let mut filter: Option<String> = None;
        let mut json = false;
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--dsl" | "-d" => {
                    if let Some(v) = it.next() {
                        dsl_root = PathBuf::from(v);
                    }
                }
                "--tests" | "-t" => {
                    if let Some(v) = it.next() {
                        tests_root = PathBuf::from(v);
                    }
                }
                "--constants" | "-c" => {
                    if let Some(v) = it.next() {
                        constants = PathBuf::from(v);
                    }
                }
                "--filter" | "-f" => {
                    if let Some(v) = it.next() {
                        filter = Some(v);
                    }
                }
                "--json" => json = true,
                "--help" | "-h" => {
                    println!(
                        "dsl-test — Ruuter DSL test runner\n\n\
                         Usage: dsl-test [--dsl DSL] [--tests DSL-tests] [--constants constants.ini] [--filter substr] [--json]"
                    );
                    std::process::exit(0);
                }
                other => {
                    eprintln!("dsl-test: unknown flag: {}", other);
                    std::process::exit(2);
                }
            }
        }
        Self {
            dsl_root,
            tests_root,
            constants,
            filter,
            json,
        }
    }
}

fn enumerate_test_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let iter = match std::fs::read_dir(dir) {
        Ok(i) => i,
        Err(_) => return,
    };
    for entry in iter.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".test.yml") || n.ends_with(".test.yaml"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

fn load_test_file(path: &Path) -> anyhow::Result<TestFile> {
    let raw = std::fs::read_to_string(path)?;
    let file: TestFile = serde_yml::from_str(&raw)?;
    Ok(file)
}

async fn run_file(
    args: &Args,
    file: &TestFile,
    file_path: &Path,
    constants: HashMap<String, String>,
) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
    match file.mode {
        Mode::Inprocess => run_inprocess(args, file, constants).await,
        Mode::MockHttp => run_mock_http(args, file, constants).await,
        Mode::WsClient => run_ws_client(args, file, constants).await,
        Mode::TriggerInject => run_trigger_inject(args, file, file_path, constants).await,
    }
}

async fn run_inprocess(
    args: &Args,
    file: &TestFile,
    constants: HashMap<String, String>,
) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
    let harness = Harness::build(&args.dsl_root, constants)?;
    let mut results = Vec::new();
    for s in &file.tests {
        let outcome = run_http_scenario(&harness, s, None).await;
        results.push(to_result(&s.name, outcome));
    }
    Ok(results)
}

async fn run_mock_http(
    args: &Args,
    file: &TestFile,
    constants: HashMap<String, String>,
) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
    // Boot a mock upstream, then merge its base URL into constants
    // under the reserved key `__MOCK__` so DSLs / test files can
    // reference it via `[#__MOCK__]`.
    let mock = MockServer::spawn().await?;
    let merged = merge_constants_with_mock(constants, mock.base_url());

    // Apply per-file http_rewrite (with {MOCK} → mock base URL).
    apply_http_rewrite(&file.http_rewrite, mock.base_url());

    let harness = Harness::build(&args.dsl_root, merged)?;

    let mut results = Vec::new();
    for s in &file.tests {
        mock.clear();
        if let Some(setup) = &s.setup {
            if !setup.mocks.is_empty() {
                mock.register(&setup.mocks);
            }
            for seed in &setup.state {
                harness
                    .state
                    .set(&seed.project, &seed.key, seed.value.clone());
            }
        }

        let outcome = run_http_scenario(&harness, s, Some(&mock)).await;
        results.push(to_result(&s.name, outcome));
    }
    mock.shutdown();
    Ok(results)
}

async fn run_trigger_inject(
    args: &Args,
    file: &TestFile,
    _file_path: &Path,
    constants: HashMap<String, String>,
) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
    let mock = MockServer::spawn().await?;
    let merged = merge_constants_with_mock(constants, mock.base_url());

    apply_http_rewrite(&file.http_rewrite, mock.base_url());

    let harness = Harness::build(&args.dsl_root, merged)?;

    let mut results = Vec::new();
    for s in &file.tests {
        mock.clear();
        if let Some(setup) = &s.setup {
            if !setup.mocks.is_empty() {
                mock.register(&setup.mocks);
            }
            for seed in &setup.state {
                harness
                    .state
                    .set(&seed.project, &seed.key, seed.value.clone());
            }
        }

        let outcome = run_trigger_scenario(&harness, s, &mock).await;
        results.push(to_result(&s.name, outcome));
    }
    mock.shutdown();
    Ok(results)
}

async fn run_ws_client(
    args: &Args,
    file: &TestFile,
    constants: HashMap<String, String>,
) -> anyhow::Result<Vec<(String, bool, Option<String>)>> {
    // Build our own router + engine + trigger dispatcher, mount them
    // as an axum app, bind on 127.0.0.1:0, and open a real WS client.
    let mut config = AppConfig::default();
    config.config_path = args.dsl_root.clone();

    let loader = DslLoader::new(config.clone(), constants);
    let loaded = loader.load_everything()?;
    let state = StateStore::new();
    let ws_registry = WsRegistry::new();
    let http_client = HttpClient::new(&config);
    let shared_http_dsls = Arc::new(loaded.http);
    let engine = StepEngine::new(http_client)
        .with_ws_registry(ws_registry.clone())
        .with_dsls(shared_http_dsls.clone());
    let _trigger = Arc::new(TriggerDispatcher::new(
        loaded.triggers,
        state.clone(),
        engine.clone(),
    ));
    let router = DslRouter::from_arc(
        shared_http_dsls,
        loaded.guards,
        config.clone(),
        state.clone(),
        ws_registry,
        engine,
    );
    let app = router.build_axum_router();

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let sd2 = shutdown.clone();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { sd2.notified().await })
            .await;
    });

    let mut results = Vec::new();
    for s in &file.tests {
        if let Some(setup) = &s.setup {
            for seed in &setup.state {
                state.set(&seed.project, &seed.key, seed.value.clone());
            }
        }
        let outcome = run_ws_scenario(addr, &state, s).await;
        results.push(to_result(&s.name, outcome));
    }
    shutdown.notify_waiters();
    let _ = server_handle.await;
    Ok(results)
}

async fn run_http_scenario(
    harness: &Harness,
    s: &Scenario,
    mock: Option<&MockServer>,
) -> Result<(), String> {
    // Seed state for inprocess mode (mock-http already handled by caller).
    if let Some(setup) = &s.setup {
        for seed in &setup.state {
            harness
                .state
                .set(&seed.project, &seed.key, seed.value.clone());
        }
    }

    let req = s
        .request
        .as_ref()
        .ok_or_else(|| "scenario missing `request:`".to_string())?;
    let expect = s.expect.clone().unwrap_or_default();

    let HarnessResponse {
        status,
        body,
        headers,
    } = harness
        .execute_http(
            &req.method,
            &req.path,
            req.body.as_ref(),
            &req.query,
            &req.headers,
        )
        .await
        .map_err(|e| format!("router execute error: {}", e))?;

    check_http_response(status, &body, &headers, &expect)?;

    verify_state(harness, s)?;
    if let Some(m) = mock {
        for a in &s.verify_mocks {
            m.assert(a)?;
        }
    }
    Ok(())
}

async fn run_ws_scenario(
    addr: std::net::SocketAddr,
    state: &StateStore,
    s: &Scenario,
) -> Result<(), String> {
    let ws = s
        .ws
        .as_ref()
        .ok_or_else(|| "scenario missing `ws:`".to_string())?;
    let url = format!("ws://{}{}", addr, ws.path);

    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("ws connect {}: {}", url, e))?;

    for frame in &ws.send {
        let text = serde_json::to_string(frame).map_err(|e| e.to_string())?;
        socket
            .send(Message::Text(text))
            .await
            .map_err(|e| format!("ws send: {}", e))?;
    }

    // Collect exactly N frames or time out.
    let want = ws.expect_frames.len();
    let timeout = Duration::from_millis(ws.timeout_ms.unwrap_or(2000));
    let mut got: Vec<Value> = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    while got.len() < want {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::String(t));
                got.push(v);
            }
            Ok(Some(Ok(Message::Binary(b)))) => {
                let text = String::from_utf8_lossy(&b).to_string();
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
                got.push(v);
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(format!("ws recv: {}", e)),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    let _ = socket.close(None).await;

    if got.len() != want {
        return Err(format!(
            "ws: expected {} frame(s), got {}\n  expected: {:?}\n  got: {:?}",
            want, got.len(), ws.expect_frames, got
        ));
    }
    for (i, expected) in ws.expect_frames.iter().enumerate() {
        if !subset_matches(expected, &got[i]) {
            return Err(format!(
                "ws frame #{} does not subset-match expected\n  expected: {}\n  actual:   {}",
                i, expected, got[i]
            ));
        }
    }

    // State assertions.
    for a in &s.verify_state {
        let actual = state.get(&a.project, &a.key);
        match (&a.value, actual) {
            (Value::Null, None) => {}
            (Value::Null, Some(v)) => {
                return Err(format!(
                    "verify_state: {}::{} expected null (missing), got {}",
                    a.project, a.key, v
                ))
            }
            (expected, Some(actual)) => {
                if !subset_matches(expected, &actual) {
                    return Err(format!(
                        "verify_state: {}::{} does not subset-match\n  expected: {}\n  actual:   {}",
                        a.project, a.key, expected, actual
                    ));
                }
            }
            (expected, None) => {
                return Err(format!(
                    "verify_state: {}::{} expected {} but key was missing",
                    a.project, a.key, expected
                ))
            }
        }
    }
    Ok(())
}

async fn run_trigger_scenario(
    harness: &Harness,
    s: &Scenario,
    mock: &MockServer,
) -> Result<(), String> {
    let t = s
        .trigger
        .as_ref()
        .ok_or_else(|| "scenario missing `trigger:`".to_string())?;
    let dispatched = harness
        .trigger
        .dispatch(&t.project, &t.channel, &t.key, t.payload.clone())
        .await
        .map_err(|e| format!("dispatch error: {}", e))?;
    if dispatched != t.expect_dispatched {
        return Err(format!(
            "trigger: expected dispatched={}, got {}",
            t.expect_dispatched, dispatched
        ));
    }

    verify_state(harness, s)?;
    for a in &s.verify_mocks {
        mock.assert(a)?;
    }
    Ok(())
}

fn verify_state(harness: &Harness, s: &Scenario) -> Result<(), String> {
    for a in &s.verify_state {
        let actual = harness.state.get(&a.project, &a.key);
        match (&a.value, actual) {
            (Value::Null, None) => {}
            (Value::Null, Some(v)) => {
                return Err(format!(
                    "verify_state: {}::{} expected null (missing), got {}",
                    a.project, a.key, v
                ))
            }
            (expected, Some(actual)) => {
                if !subset_matches(expected, &actual) {
                    return Err(format!(
                        "verify_state: {}::{} does not subset-match\n  expected: {}\n  actual:   {}",
                        a.project, a.key, expected, actual
                    ));
                }
            }
            (expected, None) => {
                return Err(format!(
                    "verify_state: {}::{} expected {} but key was missing",
                    a.project, a.key, expected
                ))
            }
        }
    }
    Ok(())
}

fn check_http_response(
    status: u16,
    body: &Value,
    headers: &HashMap<String, String>,
    expect: &ExpectHttp,
) -> Result<(), String> {
    if let Some(s) = expect.status {
        if s != status {
            return Err(format!("status: expected {}, got {} (body: {})", s, status, body));
        }
    }
    if let Some(b) = &expect.body {
        if !deep_equal(b, body) {
            return Err(format!(
                "body: expected exact match\n  expected: {}\n  actual:   {}",
                b, body
            ));
        }
    }
    if let Some(b) = &expect.body_matches {
        if !subset_matches(b, body) {
            return Err(format!(
                "body_matches: subset failed\n  expected: {}\n  actual:   {}",
                b, body
            ));
        }
    }
    for (k, v) in &expect.headers {
        match get_header_ci(headers, k) {
            Some(actual) if actual == *v => {}
            Some(actual) => {
                return Err(format!(
                    "header '{}': expected '{}', got '{}'",
                    k, v, actual
                ))
            }
            None => return Err(format!("header '{}': missing (want '{}')", k, v)),
        }
    }
    for k in &expect.header_present {
        if get_header_ci(headers, k).is_none() {
            return Err(format!("header '{}': expected present, was absent", k));
        }
    }
    for k in &expect.header_absent {
        if get_header_ci(headers, k).is_some() {
            return Err(format!("header '{}': expected absent, was present", k));
        }
    }
    if let Some(replayed) = expect.replayed {
        let hit = get_header_ci(headers, "idempotency-replayed")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if hit != replayed {
            return Err(format!(
                "replayed: expected {}, got {} (headers: {:?})",
                replayed, hit, headers
            ));
        }
    }
    Ok(())
}

fn get_header_ci(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

/// Merge constants for mock-http / trigger-inject modes: expand any
/// `{MOCK}` token in constant values with the mock's base URL, then
/// add `__MOCK__` as a first-class constant so DSLs can reference
/// `[#__MOCK__]` directly.
fn merge_constants_with_mock(
    mut constants: HashMap<String, String>,
    mock_base: &str,
) -> HashMap<String, String> {
    for v in constants.values_mut() {
        if v.contains("{MOCK}") {
            *v = v.replace("{MOCK}", mock_base);
        }
    }
    constants.insert("__MOCK__".to_string(), mock_base.to_string());
    constants
}

/// Set `RUUTER_HTTP_REWRITE` env var from a per-file map. `{MOCK}` in
/// the value is replaced with the mock server's base URL. Called
/// before Harness::build (which spins up HttpClient reading the env).
fn apply_http_rewrite(map: &HashMap<String, String>, mock_base: &str) {
    if map.is_empty() {
        std::env::remove_var("RUUTER_HTTP_REWRITE");
        return;
    }
    let pairs: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("{}={}", k, v.replace("{MOCK}", mock_base)))
        .collect();
    std::env::set_var("RUUTER_HTTP_REWRITE", pairs.join(","));
}

fn to_result(name: &str, r: Result<(), String>) -> (String, bool, Option<String>) {
    match r {
        Ok(()) => (name.to_string(), true, None),
        Err(e) => (name.to_string(), false, Some(e)),
    }
}

struct RunSummary {
    total: usize,
    passed: usize,
    failed: usize,
    items: Vec<(PathBuf, String, bool, Option<String>)>,
    json_mode: bool,
}

impl RunSummary {
    fn new(json_mode: bool) -> Self {
        Self {
            total: 0,
            passed: 0,
            failed: 0,
            items: Vec::new(),
            json_mode,
        }
    }
    fn record(&mut self, path: &Path, scenario: &str, ok: bool, err: Option<String>) {
        self.total += 1;
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        self.items
            .push((path.to_path_buf(), scenario.to_string(), ok, err));
    }
    fn emit(&self) {
        if self.json_mode {
            let items: Vec<Value> = self
                .items
                .iter()
                .map(|(p, s, ok, err)| {
                    serde_json::json!({
                        "path": p.to_string_lossy(),
                        "scenario": s,
                        "ok": ok,
                        "error": err,
                    })
                })
                .collect();
            let out = serde_json::json!({
                "total": self.total,
                "passed": self.passed,
                "failed": self.failed,
                "items": items,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            return;
        }
        for (p, name, ok, err) in &self.items {
            if *ok {
                println!("\x1b[32mpass\x1b[0m  {}::{}", p.display(), name);
            } else {
                println!(
                    "\x1b[31mfail\x1b[0m  {}::{}\n      {}",
                    p.display(),
                    name,
                    err.as_deref().unwrap_or("(no message)").replace('\n', "\n      ")
                );
            }
        }
        println!();
        println!(
            "dsl-test: {} scenario(s) — {} passed, {} failed",
            self.total, self.passed, self.failed
        );
    }
}

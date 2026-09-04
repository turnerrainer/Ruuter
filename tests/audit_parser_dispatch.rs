//! Audit finding 09 regression tests — explicit step-type dispatch,
//! reject unknown / typo'd bodies at parse time, recognise
//! `call: reflect.mock` as HttpMockStep.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write(dsl_root: &Path, rel: &str, body: &str) {
    let path = dsl_root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn try_load(dsl_root: &Path) -> Result<(), String> {
    let mut config = AppConfig::default();
    config.config_path = dsl_root.to_path_buf();
    DslLoader::new(config, HashMap::new())
        .load_everything()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Typo'd `asign:` used to parse as an empty Declaration and run as
/// silent no-op (finding 09). Now it fails at load time.
#[tokio::test]
async fn typo_asign_is_hard_load_error() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/typo.yml",
        r#"
mistyped:
  asign:
    x: 42
"#,
    );
    let err = try_load(tmp.path()).unwrap_err();
    assert!(
        err.contains("no recognised step discriminator"),
        "expected discriminator-not-found error, got: {err}"
    );
}

/// `call: reflect.mock` is now an explicit variant. The mock's
/// response is bound under the step's `result:` and downstream
/// steps see the same `{response:{status,body,headers}}` shape as
/// a real http.<verb> call.
#[tokio::test]
async fn reflect_mock_parses_and_runs() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/mocked.yml",
        r#"
mock_upstream:
  call: reflect.mock
  args:
    request:
      url: "http://real/service"
    response:
      hello: "mocked"
  result: mockResult
use_mock:
  return: "${mockResult.response.body.hello}"
  status: 200
"#,
    );
    try_load(tmp.path()).expect("reflect.mock must parse");
}

/// Unknown `call:` value is a hard load error (matches Java's
/// IllegalArgumentException path).
#[tokio::test]
async fn unknown_call_value_is_hard_load_error() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/weird.yml",
        r#"
bad:
  call: does.not.exist
  args:
    url: whatever
"#,
    );
    let err = try_load(tmp.path()).unwrap_err();
    assert!(
        err.contains("unknown call"),
        "expected unknown-call error, got: {err}"
    );
}

/// A step body that IS just a control-flow shell (only `next:`)
/// still parses — pre-existing tests use this shape as a jump
/// target. Treated as an implicit no-op Declaration.
#[tokio::test]
async fn control_flow_only_step_parses_as_noop() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/hop.yml",
        r#"
hop:
  next: done
done:
  return: "arrived"
  status: 200
"#,
    );
    try_load(tmp.path()).expect("bare next-only step must parse");
}

// Issue #56 — one action key per step, enforced at parse time.
//
// The parser used to dispatch by if-ladder priority (`call:` beats
// `template:` beats `assign:` beats … beats `log:`) and let serde
// silently drop every non-winning key. A DSL author who wrote
// `log:` alongside `call:` in one step got the http call and had
// their log message deleted from memory with no signal. Now every
// such step is a hard load error naming both keys.

#[tokio::test]
async fn multi_action_log_plus_call_is_hard_load_error() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/mix.yml",
        r#"
bad:
  log: "before the call"
  call: http.get
  args:
    url: "http://example.com"
"#,
    );
    let err = try_load(tmp.path()).unwrap_err();
    assert!(
        err.contains("declares 2 actions in one step"),
        "expected multi-action rejection with count, got: {err}"
    );
    assert!(err.contains("call") && err.contains("log"), "error must name BOTH offending keys, got: {err}");
}

#[tokio::test]
async fn multi_action_assign_plus_switch_is_hard_load_error() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/mix2.yml",
        r#"
bad:
  assign:
    x: 1
  switch:
    - condition: "true"
      next: end
"#,
    );
    let err = try_load(tmp.path()).unwrap_err();
    assert!(
        err.contains("declares 2 actions in one step"),
        "expected multi-action rejection with count, got: {err}"
    );
    assert!(err.contains("assign") && err.contains("switch"), "error must name BOTH offending keys, got: {err}");
}

/// Regression guard: a step containing exactly ONE action key plus
/// unrelated non-action keys (base fields like `next:`, `sleep:`,
/// Declaration metadata like `description:`) still parses. The
/// multi-discriminator check counts only action keys.
#[tokio::test]
async fn single_action_with_base_and_metadata_still_parses() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/fine.yml",
        r#"
say:
  log: "hi"
  sleep: 0
  next: end
"#,
    );
    try_load(tmp.path()).expect("single-action step must still parse");
}

/// `call: declare` is the Java-parity way to opt into a Declaration
/// step, and Declaration bodies legitimately carry many keys
/// (`version:`, `description:`, `namespace:`, `allowlist:`, …).
/// None of those are action keys, so the check does not fire.
#[tokio::test]
async fn declaration_via_call_declare_still_parses() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/GET/dec.yml",
        r#"
info:
  call: declare
  version: "1.0"
  description: "hello"
say:
  return: "world"
  status: 200
"#,
    );
    try_load(tmp.path()).expect("call:declare with metadata must still parse");
}

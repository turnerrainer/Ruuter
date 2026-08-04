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

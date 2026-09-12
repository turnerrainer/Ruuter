//! h2ck.me v1 T-11 — `ruuter-doctor` pre-boot config sanity check.
//!
//! Ops teams have `dsl-lint` and `dsl-test` for DSL-side; nothing
//! for runtime config. The doctor loads ruuter.yaml + env vars and
//! runs the exact WARN registry the boot path uses. Exit code is
//! actionable in CI:
//!
//! - 0 → clean; ok to ship.
//! - 1 → at least one WARN would fire at boot (unsafe default,
//!       stale-config field set, missing OWASP baseline on
//!       non-loopback bind, etc.). Diagnostic mode of CI.
//! - 2 → config file unreadable / unparseable (typo, missing).
//! - 3 → invalid CLI arguments.
//!
//! Tests written to try to BREAK the exit-code contract:
//! - Clean config → 0, "config is clean".
//! - Config with `stop_in_case_of_exception: false` → 1, WARN
//!   captured on stdout.
//! - Unparseable YAML → 2 with error text on stderr.
//! - Non-existent path → 2.
//! - Unknown CLI flag → 3.
//! - --help → 0.

#![allow(clippy::field_reassign_with_default)]

use std::path::PathBuf;
use std::process::Command;

fn doctor_path() -> PathBuf {
    // Cargo drops binaries at $CARGO_TARGET_DIR/debug/<name>. When
    // CARGO_TARGET_DIR is unset, uses ./target relative to the
    // workspace root. We resolve both.
    let candidate1 = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(|d| PathBuf::from(d).join("debug/ruuter-doctor"));
    let candidate2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/ruuter-doctor");
    if let Some(p) = candidate1 {
        if p.exists() {
            return p;
        }
    }
    candidate2
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_doctor(args: &[&str]) -> (i32, String, String) {
    let path = doctor_path();
    assert!(
        path.exists(),
        "ruuter-doctor binary must be built before this test runs; \
         expected at {}",
        path.display()
    );
    let output = Command::new(&path)
        .args(args)
        .output()
        .expect("spawn ruuter-doctor");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (code, stdout, stderr)
}

#[test]
fn help_flag_exits_zero() {
    let (code, stdout, _) = run_doctor(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("ruuter-doctor"));
    assert!(stdout.contains("Exit codes"));
}

#[test]
fn unknown_flag_exits_three() {
    let (code, _, stderr) = run_doctor(&["--nope"]);
    assert_eq!(code, 3);
    assert!(
        stderr.contains("unknown argument"),
        "stderr must name the argument problem; got:\n{stderr}"
    );
}

#[test]
fn missing_config_path_exits_two() {
    let (code, _, stderr) = run_doctor(&["--config", "/nonexistent/path/ruuter.yaml"]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("config load failed"),
        "stderr must name the load failure; got:\n{stderr}"
    );
}

#[test]
fn unparseable_config_exits_two() {
    let p = fixture("unparseable.yaml");
    assert!(p.exists(), "test fixture missing: {}", p.display());
    let (code, _, stderr) = run_doctor(&["--config", p.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("config load failed"));
}

#[test]
fn clean_config_exits_zero_with_no_warnings_line() {
    let p = fixture("clean-defaults.yaml");
    let (code, stdout, _) = run_doctor(&["--config", p.to_str().unwrap()]);
    assert_eq!(code, 0, "clean config must exit 0; stdout:\n{stdout}");
    assert!(
        stdout.contains("0 warnings"),
        "clean config must report 0 warnings; got:\n{stdout}"
    );
    assert!(
        stdout.contains(p.to_str().unwrap()),
        "stdout must name the config source path; got:\n{stdout}"
    );
}

#[test]
fn insecure_defaults_config_exits_one_with_warn() {
    let p = fixture("insecure-defaults.yaml");
    let (code, stdout, _) = run_doctor(&["--config", p.to_str().unwrap()]);
    assert_eq!(
        code, 1,
        "warning-eligible config must exit 1; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("stop_in_case_of_exception"),
        "captured WARN must name the field; got:\n{stdout}"
    );
    // Should also count the warning.
    assert!(
        stdout.contains("warning(s):") || stdout.contains("1 warning"),
        "stdout must report the warning count; got:\n{stdout}"
    );
}

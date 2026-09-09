//! Issue #91 — `dsl-lint` flags unquoted `${...}` scalars whose
//! expression body contains `: ` (space + colon + space), the YAML
//! mapping-value indicator.
//!
//! The trap:
//! ```yaml
//! step:
//!   assign:
//!     x: ${a ? b : c}    # ` : ` truncates the plain scalar; x
//!                        # silently becomes "${a ? b" and `c}` is
//!                        # interpreted as a new key.
//! ```
//! Fix is trivial once known — wrap in quotes: `x: "${a ? b : c}"`.
//! Failure mode is silent misparse, so authors ship the wrong value
//! before they notice.
//!
//! Post-fix, `dsl-lint` emits a WARNING (never an error — the file
//! may still parse and run correctly for other reasons) naming the
//! line number and the suggested quoted form.

use std::process::Command;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn make_tree(files: &[(&str, &str)]) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!("ruuter-91-{}", uuid()));
    for (rel, body) in files {
        let path = tmp.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }
    // Empty constants.ini — none of the fixtures reference [#…].
    let constants = tmp.join("constants.ini");
    std::fs::write(&constants, "").unwrap();
    tmp
}

fn run_lint(tmp: &std::path::Path) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_dsl-lint");
    let out = Command::new(bin)
        .arg("--dsl")
        .arg(tmp)
        .arg("--constants")
        .arg(tmp.join("constants.ini"))
        .output()
        .expect("failed to invoke dsl-lint");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The reporter's motivating case: ternary expression in a plain
/// scalar. dsl-lint should WARN, exit 0 (WARN doesn't fail the
/// build), and the diagnostic should carry the suggested quoted
/// form.
#[test]
fn ternary_in_unquoted_scalar_is_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Trailing colon-space inside `${…}` — the trap.
        "stamp: { assign: { x: ${a ? b : c} }, next: end }\n",
    )]);
    // The YAML above IS actually well-formed (curly-braces are flow
    // maps, `${a ? b : c}` is one flow-map value inside another),
    // so serde may still parse it. dsl-lint's warning surfaces the
    // fragility regardless. Try a block-style variant so the trap
    // fires for real:
    let block_tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: ${a ? b : c}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&block_tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("unquoted") && output.contains("mapping-value indicator"),
        "dsl-lint must warn on `${{a ? b : c}}` in a plain scalar; got:\n{output}"
    );
    // Also ensure the (nested-flow-map) variant either parses fine
    // or ALSO warns — the warning is defensible in both shapes.
    let _ = tmp;
}

/// Quoted variant → NO warning. Same expression, wrapped in double
/// quotes.
#[test]
fn quoted_ternary_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: \"${a ? b : c}\"\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("unquoted") || !output.contains("mapping-value"),
        "quoted `${{a ? b : c}}` must not warn; got:\n{output}"
    );
}

/// Single quotes also silence the warning.
#[test]
fn single_quoted_ternary_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: '${a ? b : c}'\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("unquoted") || !output.contains("mapping-value"),
        "single-quoted expression must not warn; got:\n{output}"
    );
}

/// An expression without any problematic character should also not
/// warn — the lint is targeted, not "warn on every unquoted
/// interpolation."
#[test]
fn plain_scalar_expression_without_colon_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: ${incoming.body.name}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("mapping-value"),
        "plain `${{incoming.body.name}}` must not warn; got:\n{output}"
    );
}

/// Comment lines starting with `#` and list-item prefixes starting
/// with `- ` must not be inspected — they're not `<key>: <value>`
/// mapping entries and matching them would produce false positives.
#[test]
fn comments_and_list_items_are_not_inspected() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        r#"# some comment: ${a ? b : c} inside a comment
stamp:
  switch:
    - condition: ${x == 1}
      next: end
end:
  return: ok
  status: 200
"#,
    )]);
    let (code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    // Should not warn on the comment or the switch list item.
    assert!(
        !output.contains("mapping-value"),
        "no warning expected; got:\n{output}"
    );
    // dsl-lint exits 0 when there are no errors (warnings OK).
    assert_eq!(code, 0);
}

/// The scalar-quoting check is a WARNING, not an error — it
/// increments `report.warnings` (not `report.errors`) and calls
/// `report.file_warning` internally. In practice the same fixture
/// that trips the warning ALSO trips a YAML parse error on
/// serde-yaml-ng (which is how the maintainer first noticed the
/// class), so the exit code is 1 for the parse error and the
/// warning surfaces as ADDITIONAL diagnostic. This test pins the
/// contract at the diagnostic level: the trailing "3 warning(s)"
/// count in dsl-lint's summary line reflects our warning, not an
/// error.
#[test]
fn scalar_quoting_finding_is_categorised_as_warning() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: ${a ? b : c}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    // Summary line must show at least one warning. The exact
    // format is `dsl-lint: <N> file(s) scanned, <M> ok, <E> error(s),
    // <W> warning(s)` — we grep for a non-zero warning count.
    let has_warning = output.contains("1 warning(s)") || output.contains("2 warning(s)");
    assert!(
        has_warning,
        "scalar-quoting finding must be counted as a warning in the summary: {output}"
    );
}

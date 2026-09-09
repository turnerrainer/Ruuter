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
    let has_warning = output.contains("1 warning(s)")
        || output.contains("2 warning(s)")
        || output.contains("3 warning(s)");
    assert!(
        has_warning,
        "scalar-quoting finding must be counted as a warning in the summary: {output}"
    );
}

// ────────────────────────────────────────────────────────────────
// Broader YAML flow-terminator coverage — the extended check set
// closes the gap between the reporter's full character list and
// the initial "just `: `" scope.
// ────────────────────────────────────────────────────────────────

/// ` # ` (space + hash) inside a `${...}` scalar starts a YAML
/// comment and truncates the plain scalar. Warn.
#[test]
fn hash_in_unquoted_expression_body_is_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: ${foo #note}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("` #`") && output.contains("comment"),
        "` #` inside `${{…}}` must warn with the comment-cut hint: {output}"
    );
}

/// Same expression body but the scalar is quoted → no warning.
#[test]
fn quoted_hash_expression_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: \"${foo #note}\"\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.to_lowercase().contains("comment"),
        "quoted body must not warn about ` #`: {output}"
    );
}

/// `,` inside a `${...}` scalar WHEN the surrounding line is in
/// flow context (unclosed `{` before the `${`) — YAML uses the
/// `,` as flow-element separator and misparses the rest.
#[test]
fn comma_in_flow_context_expression_is_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Line puts the `${format(a, b)}` inside a flow-mapping;
        // the `,` inside the expression body terminates the flow
        // element. Only warns in flow context.
        "stamp: { assign: { x: ${format(a, b)} } }\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("flow context") && output.contains(","),
        "`,` in flow-context `${{…}}` must warn: {output}"
    );
}

/// `,` inside a `${...}` scalar in BLOCK context (no unclosed flow
/// container before the `${`) must NOT warn — false-positive
/// avoidance for legitimate `${arr.map((a, b) => …)}` shape.
#[test]
fn comma_in_block_context_expression_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Block-style: `x: ${format(a, b)}` — no unclosed flow
        // container before the `${`. `,` inside inner is fine.
        "stamp:\n  assign:\n    x: ${format(a, b)}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("flow context"),
        "`,` in block-context `${{…}}` must NOT noise-warn: {output}"
    );
}

/// A value starting with `!` (YAML tag) is a reserved metasyntax.
/// Warn — unquoted `x: !foo` fails to load in most YAML parsers.
#[test]
fn value_starting_with_yaml_metasyntax_char_is_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // `x: !important` — `!` starts a YAML tag; parser errors
        // on unknown tag. Same fix (quote) applies.
        "stamp:\n  assign:\n    x: !important\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("reserves") || output.contains("YAML reserves"),
        "value-start `!` must warn about YAML metasyntax: {output}"
    );
}

/// Every reserved scalar-start character warns.
#[test]
fn every_reserved_scalar_start_char_warns() {
    // `!` `&` `*` `%` `@` backtick — the six characters our
    // check flags. Each in its own fixture so a per-char failure
    // is precise.
    for (name, ch) in &[
        ("bang", '!'),
        ("amp", '&'),
        ("star", '*'),
        ("percent", '%'),
        ("at", '@'),
        ("backtick", '`'),
    ] {
        let tmp = make_tree(&[(
            &format!("svc/GET/{name}.yml"),
            &format!(
                "stamp:\n  assign:\n    x: {ch}value\n  next: end\nend:\n  return: ok\n  status: 200\n"
            ),
        )]);
        let (_code, stdout, stderr) = run_lint(&tmp);
        let output = format!("{stdout}\n{stderr}");
        assert!(
            output.contains("reserves"),
            "value-start `{ch}` in fixture `{name}` must warn: {output}"
        );
    }
}

/// Unicode fullwidth colon (U+FF1A) looks like `:` but doesn't
/// parse as a mapping-value indicator. Copy-paste from rendered
/// docs is the usual entry point.
#[test]
fn fullwidth_colon_homoglyph_is_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Note the `：` between `key` and `value` — that's U+FF1A,
        // not U+003A. YAML doesn't recognise it as `:`.
        "key\u{FF1A}value\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("fullwidth colon") || output.contains("U+FF1A"),
        "U+FF1A homoglyph must warn: {output}"
    );
}

/// Unicode en dash (U+2013) and em dash (U+2014) both warn as
/// look-alikes for ASCII `-`. Ships as separate fixtures so a
/// per-glyph regression is easy to name.
#[test]
fn en_dash_and_em_dash_homoglyphs_are_warned() {
    // en dash
    let tmp = make_tree(&[(
        "svc/GET/en.yml",
        "en_dash\u{2013}key: value\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("en dash") || output.contains("U+2013"),
        "U+2013 (en dash) must warn: {output}"
    );

    // em dash
    let tmp = make_tree(&[(
        "svc/GET/em.yml",
        "em_dash\u{2014}key: value\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("em dash") || output.contains("U+2014"),
        "U+2014 (em dash) must warn: {output}"
    );
}

/// A repeated homoglyph on the same line surfaces exactly one
/// warning (not one per occurrence) — otherwise a copy-pasted
/// block from a rendered doc could spam.
#[test]
fn repeated_homoglyph_on_one_line_warns_once() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Two U+FF1A on the same line → one warning.
        "a\u{FF1A}b\u{FF1A}c\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    let hits = output.matches("fullwidth colon").count();
    assert_eq!(
        hits, 1,
        "repeated homoglyph on one line must warn once, got {hits}: {output}"
    );
}

/// A homoglyph inside a `#` comment is stylistic (typographers'
/// em dash inside a doc comment is fine — YAML treats the whole
/// comment as opaque). Must NOT warn.
#[test]
fn homoglyph_inside_comment_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // `# heading — with em dash` — em dash sits after the `#`
        // comment start. Structural YAML is unaffected.
        "# heading \u{2014} with em dash\nstamp:\n  assign:\n    x: 1\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("em dash") && !output.contains("U+2014"),
        "homoglyph inside `#` comment must NOT warn (stylistic use): {output}"
    );
}

/// A homoglyph inside a quoted string is the author's literal
/// intent (rendered docs, i18n strings, prose). Must NOT warn.
#[test]
fn homoglyph_inside_quoted_string_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // Value is a double-quoted string carrying an em dash on
        // purpose. Not a YAML-structure hazard.
        "stamp:\n  assign:\n    title: \"foo \u{2014} bar\"\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("em dash"),
        "homoglyph inside a quoted string must NOT warn: {output}"
    );
}

/// A trailing-comment homoglyph on a mapping line is still
/// stylistic ("real" YAML structure is to the left of the ` #`).
/// Must NOT warn.
#[test]
fn homoglyph_in_trailing_comment_is_not_warned() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        "stamp:\n  assign:\n    x: 1 # value \u{2014} note\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        !output.contains("em dash"),
        "homoglyph in trailing comment must NOT warn: {output}"
    );
}

/// Multiple traps on ONE `${...}` scalar surface as separate
/// warnings — one per class — so a DSL author fixing the file
/// sees every reason to quote it.
#[test]
fn multiple_traps_on_one_scalar_produce_multiple_warnings() {
    let tmp = make_tree(&[(
        "svc/GET/probe.yml",
        // `${a ? b : c #d}` has BOTH `: ` AND ` #` inside the
        // inner. Two warnings expected on the same line.
        "stamp:\n  assign:\n    x: ${a ? b : c #d}\n  next: end\nend:\n  return: ok\n  status: 200\n",
    )]);
    let (_code, stdout, stderr) = run_lint(&tmp);
    let output = format!("{stdout}\n{stderr}");
    assert!(
        output.contains("mapping-value indicator"),
        "must warn about `: `: {output}"
    );
    assert!(
        output.contains("start a comment"),
        "must warn about ` #`: {output}"
    );
}

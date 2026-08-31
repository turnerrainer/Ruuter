//! dsl-lint — static validator for the Ruuter DSL tree.
//!
//! Walks every `*.yml` / `*.yaml` under a root directory and reports:
//!
//! - YAML parse failure
//! - Step type unrecognised
//! - `next:` reference that resolves to no step in the same DSL
//! - Empty DSL (no steps)
//! - Unresolved `[#constant]` references (against constants.ini +
//!   optional overrides)
//! - Templates that reference a DSL key that doesn't exist in the tree
//! - Reachability: steps that no other step transitions to and are not
//!   the entry step (warning, not error)
//!
//! Exits with status 1 on any error; status 0 on clean.
//!
//! Usage:
//!   dsl-lint                          # ./DSL, ./constants.ini
//!   dsl-lint --dsl DSL                # explicit root
//!   dsl-lint --dsl DSL --constants constants.ini
//!   dsl-lint --dsl DSL --include-disabled
//!   dsl-lint --json                   # machine-readable output

use indexmap::IndexMap;
use serde_json::Value;
use serde_yaml_ng::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const KNOWN_STEP_KEYS: &[&str] = &[
    "assign",
    "return",
    "call",
    "switch",
    "log",
    "template",
    "state",
    "iterate",
    "ws_send",
    "single_flight",
    "declaration",
];

fn main() -> ExitCode {
    let args = Args::parse();

    let mut report = Report::new(args.json);

    // Load constants for [#...] resolution.
    let constants = match std::fs::read_to_string(&args.constants) {
        Ok(_) => match ruuter_on_rust::config::load_constants(&args.constants) {
            Ok(c) => c,
            Err(e) => {
                report.file_error(
                    &PathBuf::from(&args.constants),
                    format!("failed to read constants: {}", e),
                );
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(),
    };

    // Enumerate DSL files.
    let files = enumerate_dsl_files(&args.dsl_root, args.include_disabled);
    report.files_scanned = files.len();

    // First pass: parse each file, collect step graph.
    let mut parsed: BTreeMap<PathBuf, ParsedFile> = BTreeMap::new();
    for path in &files {
        // Sources & cron jobs are shape-validated separately — don't
        // reject their per-key values as "not a step mapping".
        let strict = !is_source_file(path) && !is_cron_job_file(path);
        match parse_file(path, &constants, strict) {
            Ok(pf) => {
                parsed.insert(path.clone(), pf);
            }
            Err(errs) => {
                for e in errs {
                    report.file_error(path, e);
                }
            }
        }
    }

    // Build a set of "known DSL keys" for template validation. Layout
    // mirrors the runtime loader (project/METHOD/path).
    let dsl_keys = build_dsl_key_index(&args.dsl_root, &parsed);

    // Second pass: per-file semantic checks.
    for (path, pf) in &parsed {
        if is_source_file(path) {
            check_source_shape(path, pf, &mut report);
            continue;
        }
        if is_cron_job_file(path) {
            check_cron_job_shape(path, pf, &mut report);
            continue;
        }
        check_dsl(path, pf, &constants, &dsl_keys, &mut report);
    }

    report.emit();
    if report.errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Debug)]
struct Args {
    dsl_root: PathBuf,
    constants: String,
    include_disabled: bool,
    json: bool,
}

impl Args {
    fn parse() -> Self {
        let mut dsl_root = PathBuf::from("./DSL");
        let mut constants = String::from("./constants.ini");
        let mut include_disabled = false;
        let mut json = false;
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--dsl" | "-d" => {
                    if let Some(v) = it.next() {
                        dsl_root = PathBuf::from(v);
                    }
                }
                "--constants" | "-c" => {
                    if let Some(v) = it.next() {
                        constants = v;
                    }
                }
                "--include-disabled" => include_disabled = true,
                "--json" => json = true,
                "--help" | "-h" => {
                    println!(
                        "dsl-lint — static validator for the Ruuter DSL tree\n\n\
                         Usage: dsl-lint [--dsl DSL] [--constants constants.ini] [--include-disabled] [--json]"
                    );
                    std::process::exit(0);
                }
                other => {
                    eprintln!("dsl-lint: unknown flag: {}", other);
                    std::process::exit(2);
                }
            }
        }
        Self {
            dsl_root,
            constants,
            include_disabled,
            json,
        }
    }
}

fn enumerate_dsl_files(root: &Path, include_disabled: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out, include_disabled);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, include_disabled: bool) {
    let iter = match std::fs::read_dir(dir) {
        Ok(i) => i,
        Err(_) => return,
    };
    for entry in iter.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out, include_disabled);
        } else if is_dsl_file(&p, include_disabled) {
            out.push(p);
        }
    }
}

fn is_dsl_file(p: &Path, include_disabled: bool) -> bool {
    let name = match p.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if name == ".guard" || name == ".guard.yml" || name == ".guard.yaml" {
        return true;
    }
    if include_disabled && name.ends_with(".yml.disabled") {
        return true;
    }
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "yml" || e == "yaml")
        .unwrap_or(false)
}

fn is_source_file(p: &Path) -> bool {
    // Legacy layout: <project>/sources/…
    // New layout:    <project>/WS/outbound/…
    let comps: Vec<_> = p.components().map(|c| c.as_os_str().to_owned()).collect();
    if comps.iter().any(|c| c == "sources") {
        return true;
    }
    comps.windows(2).any(|w| w[0] == "WS" && w[1] == "outbound")
}

fn is_cron_job_file(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str() == "cronmanager-jobs")
}

struct ParsedFile {
    entry_step: Option<String>,
    /// Ordered list of (step_name, step_kind, next_target_if_any)
    steps: Vec<ParsedStep>,
    /// Verbatim YAML for constant scanning.
    raw: String,
    /// Template targets referenced by the DSL (raw string).
    template_targets: Vec<String>,
}

struct ParsedStep {
    name: String,
    kind: String,
    /// Static next: target if declared.
    next: Option<String>,
    /// switch: [(condition, next-target)]
    switch_targets: Vec<String>,
}

fn parse_file(
    path: &Path,
    constants: &HashMap<String, String>,
    strict: bool,
) -> Result<ParsedFile, Vec<String>> {
    let raw = std::fs::read_to_string(path).map_err(|e| vec![format!("read error: {}", e)])?;

    let substituted = substitute_constants(&raw, constants);

    let map: IndexMap<String, YamlValue> = serde_yaml_ng::from_str(&substituted)
        .map_err(|e| vec![format!("YAML parse error: {}", e)])?;

    let mut errs = Vec::new();
    let mut steps = Vec::new();
    let mut entry_step: Option<String> = None;
    let mut template_targets = Vec::new();

    for (idx, (name, value)) in map.iter().enumerate() {
        let YamlValue::Mapping(m) = value else {
            if strict {
                errs.push(format!(
                    "step '{}': body is not a mapping (got: {})",
                    name,
                    yaml_kind(value)
                ));
            }
            continue;
        };

        // Which step kind is this? First match wins.
        let mut kind = String::from("unknown");
        for &k in KNOWN_STEP_KEYS {
            if m.contains_key(YamlValue::String(k.to_string())) {
                kind = k.to_string();
                break;
            }
        }

        // Recognition is enforced later in check_dsl (which knows to
        // skip source/cron files that aren't step DSLs).

        let next = m
            .get(YamlValue::String("next".to_string()))
            .and_then(|v| v.as_str().map(String::from));

        let mut switch_targets = Vec::new();
        if kind == "switch" {
            if let Some(YamlValue::Sequence(seq)) = m.get(YamlValue::String("switch".to_string())) {
                for cond in seq {
                    if let YamlValue::Mapping(cm) = cond {
                        if let Some(YamlValue::String(t)) =
                            cm.get(YamlValue::String("next".to_string()))
                        {
                            switch_targets.push(t.clone());
                        }
                    }
                }
            }
        }

        if kind == "template" {
            if let Some(YamlValue::String(t)) = m.get(YamlValue::String("template".to_string())) {
                template_targets.push(t.clone());
            }
        }

        if idx == 0 {
            entry_step = Some(name.clone());
        }

        steps.push(ParsedStep {
            name: name.clone(),
            kind,
            next,
            switch_targets,
        });
    }

    if !errs.is_empty() {
        return Err(errs);
    }

    Ok(ParsedFile {
        entry_step,
        steps,
        raw,
        template_targets,
    })
}

fn substitute_constants(content: &str, constants: &HashMap<String, String>) -> String {
    // Both `[#NAME]` and `#{NAME}` are handled (task 067).
    ruuter_on_rust::dsl::interpolate::substitute(content, |k| constants.get(k).cloned())
}

fn yaml_kind(v: &YamlValue) -> &'static str {
    match v {
        YamlValue::Null => "null",
        YamlValue::Bool(_) => "bool",
        YamlValue::Number(_) => "number",
        YamlValue::String(_) => "string",
        YamlValue::Sequence(_) => "sequence",
        YamlValue::Mapping(_) => "mapping",
        YamlValue::Tagged(_) => "tagged",
    }
}

fn build_dsl_key_index(
    dsl_root: &Path,
    parsed: &BTreeMap<PathBuf, ParsedFile>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in parsed.keys() {
        if let Some(key) = dsl_key_for(dsl_root, path) {
            out.insert(key);
        }
    }
    out
}

/// Given /.../DSL/samples/GET/basic/hello.yml with root /.../DSL,
/// returns Some("samples/GET/basic/hello"). The template step calls it
/// with a value like "samples/GET/basic/hello" — see template.rs.
fn dsl_key_for(dsl_root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(dsl_root).ok()?;
    let mut s = rel.to_string_lossy().to_string();
    // Strip extensions.
    for ext in [".yml", ".yaml", ".yml.disabled", ".yaml.disabled"] {
        if let Some(base) = s.strip_suffix(ext) {
            s = base.to_string();
            break;
        }
    }
    // Skip guards from the template-target set.
    if s.ends_with(".guard") || s.contains(".guard") || s.ends_with("/.guard") {
        return None;
    }
    Some(s)
}

fn check_dsl(
    path: &Path,
    pf: &ParsedFile,
    _constants: &HashMap<String, String>,
    dsl_keys: &BTreeSet<String>,
    report: &mut Report,
) {
    // Empty file? (No steps besides declaration.)
    let real_steps: Vec<&ParsedStep> = pf
        .steps
        .iter()
        .filter(|s| s.kind != "declaration")
        .collect();
    if real_steps.is_empty() && pf.template_targets.is_empty() {
        // A guard-file that only says `declaration: { override_ancestors: true }`
        // and a `deny:` step is real. The check is: at least one non-declaration
        // step must exist. If not, error.
        report.file_error(path, "DSL has no executable steps".into());
        return;
    }

    // Unrecognised step kinds (moved from parse_file so source/cron
    // files aren't held to this check).
    for s in &pf.steps {
        if s.kind == "unknown" && s.name != "declaration" {
            report.file_error(
                path,
                format!(
                    "step '{}': unrecognised step (expected one of: {})",
                    s.name,
                    KNOWN_STEP_KEYS.join(", ")
                ),
            );
        }
    }

    let step_names: BTreeSet<String> = pf.steps.iter().map(|s| s.name.clone()).collect();

    // next: reference resolution.
    for s in &pf.steps {
        if let Some(next) = &s.next {
            if next != "end" && !step_names.contains(next) {
                report.file_error(
                    path,
                    format!(
                        "step '{}': next target '{}' does not resolve to any step in this DSL",
                        s.name, next
                    ),
                );
            }
        }
        for t in &s.switch_targets {
            if t != "end" && !step_names.contains(t) {
                report.file_error(
                    path,
                    format!(
                        "step '{}': switch branch next '{}' does not resolve",
                        s.name, t
                    ),
                );
            }
        }
    }

    // Reachability from the entry step.
    if let Some(entry) = &pf.entry_step {
        let mut reachable = BTreeSet::new();
        walk_reachable(entry, &pf.steps, &mut reachable);
        for s in &pf.steps {
            if !reachable.contains(&s.name) && s.kind != "declaration" {
                report.file_warning(
                    path,
                    format!("step '{}' is not reachable from entry '{}'", s.name, entry),
                );
            }
        }
    }

    // Unresolved constant references — post-substitution, any surviving
    // `[#NAME]` or `#{NAME}` literal means the key wasn't in
    // constants.ini. Both syntaxes are surfaced with the exact form
    // the author used, so error messages show `[#api_key]` when they
    // wrote `[#api_key]` and `#{api_key}` when they wrote that.
    let after = substitute_constants(&pf.raw, _constants);
    for r in ruuter_on_rust::dsl::interpolate::iter_refs(&after) {
        report.file_warning(
            path,
            format!(
                "unresolved constant reference '{}' — add to constants.ini or the runner will forward the literal to reqwest",
                r.literal
            ),
        );
    }

    // Template targets must exist in the loaded DSL tree.
    for t in &pf.template_targets {
        // The template step accepts either the exact key or a
        // convention-based key path. We check both.
        let matches = dsl_keys
            .iter()
            .any(|k| k == t || k.ends_with(t) || k == t.trim_start_matches('/'));
        if !matches {
            report.file_error(
                path,
                format!(
                    "template step references '{}' but no DSL with that key was found",
                    t
                ),
            );
        }
    }

    report.files_ok += 1;
}

fn walk_reachable(start: &str, steps: &[ParsedStep], out: &mut BTreeSet<String>) {
    if !out.insert(start.to_string()) {
        return;
    }
    if let Some(s) = steps.iter().find(|s| s.name == start) {
        if let Some(n) = &s.next {
            if n != "end" {
                walk_reachable(n, steps, out);
            }
        }
        for t in &s.switch_targets {
            if t != "end" {
                walk_reachable(t, steps, out);
            }
        }
    }
    // Steps without a next: implicitly transition to the next step in
    // source order (see StepEngine::run). Model that too.
    if let Some(idx) = steps.iter().position(|s| s.name == start) {
        if let Some(next_in_order) = steps.get(idx + 1) {
            if !out.contains(&next_in_order.name)
                && steps[idx].next.is_none()
                && steps[idx].kind != "return"
            {
                walk_reachable(&next_in_order.name, steps, out);
            }
        }
    }
}

fn check_source_shape(path: &Path, pf: &ParsedFile, report: &mut Report) {
    // sources/ files must have a top-level `kind:` field. We can't run
    // them through the DSL parser, so just verify shape.
    let doc: Result<Value, _> = serde_yaml_ng::from_str(&pf.raw).map(yaml_to_json);
    let doc = match doc {
        Ok(v) => v,
        Err(e) => {
            report.file_error(path, format!("YAML parse error: {}", e));
            return;
        }
    };
    if let Some(kind) = doc.get("kind").and_then(|v| v.as_str()) {
        if kind != "websocket" {
            report.file_warning(
                path,
                format!(
                    "source kind '{}' is not currently supported (only 'websocket')",
                    kind
                ),
            );
        }
    } else {
        report.file_error(path, "source is missing required 'kind' field".into());
    }
    report.files_ok += 1;
}

fn check_cron_job_shape(path: &Path, pf: &ParsedFile, report: &mut Report) {
    // Each top-level key must be a job with at least `trigger`, `type`,
    // `url`. This is the CronManager format, not the Ruuter DSL format.
    let doc: Result<Value, _> = serde_yaml_ng::from_str(&pf.raw).map(yaml_to_json);
    let doc = match doc {
        Ok(v) => v,
        Err(e) => {
            report.file_error(path, format!("YAML parse error: {}", e));
            return;
        }
    };
    let obj = match doc.as_object() {
        Some(o) => o,
        None => {
            report.file_error(
                path,
                "cron-job file: top level must be a mapping of jobs".into(),
            );
            return;
        }
    };
    for (name, job) in obj {
        let job = match job.as_object() {
            Some(o) => o,
            None => {
                report.file_error(path, format!("cron-job '{}': body must be a mapping", name));
                continue;
            }
        };
        for required in ["trigger", "type", "url"] {
            if !job.contains_key(required) {
                report.file_error(
                    path,
                    format!("cron-job '{}': missing required field '{}'", name, required),
                );
            }
        }
    }
    report.files_ok += 1;
}

fn yaml_to_json(v: YamlValue) -> Value {
    match v {
        YamlValue::Null => Value::Null,
        YamlValue::Bool(b) => Value::Bool(b),
        YamlValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        YamlValue::String(s) => Value::String(s),
        YamlValue::Sequence(s) => Value::Array(s.into_iter().map(yaml_to_json).collect()),
        YamlValue::Mapping(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                let key = k
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| format!("{:?}", k));
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        YamlValue::Tagged(t) => yaml_to_json(t.value),
    }
}

#[derive(Debug)]
struct Report {
    files_scanned: usize,
    files_ok: usize,
    errors: usize,
    warnings: usize,
    items: Vec<Item>,
    json_mode: bool,
}

#[derive(Debug)]
struct Item {
    severity: Severity,
    path: PathBuf,
    message: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

impl Report {
    fn new(json_mode: bool) -> Self {
        Self {
            files_scanned: 0,
            files_ok: 0,
            errors: 0,
            warnings: 0,
            items: Vec::new(),
            json_mode,
        }
    }
    fn file_error(&mut self, path: &Path, message: String) {
        self.errors += 1;
        self.items.push(Item {
            severity: Severity::Error,
            path: path.to_path_buf(),
            message,
        });
    }
    fn file_warning(&mut self, path: &Path, message: String) {
        self.warnings += 1;
        self.items.push(Item {
            severity: Severity::Warning,
            path: path.to_path_buf(),
            message,
        });
    }
    fn emit(&self) {
        if self.json_mode {
            let items: Vec<Value> = self
                .items
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "severity": if i.severity == Severity::Error { "error" } else { "warning" },
                        "path": i.path.to_string_lossy(),
                        "message": i.message,
                    })
                })
                .collect();
            let out = serde_json::json!({
                "files_scanned": self.files_scanned,
                "files_ok": self.files_ok,
                "errors": self.errors,
                "warnings": self.warnings,
                "items": items,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
            return;
        }
        for i in &self.items {
            let tag = match i.severity {
                Severity::Error => "\x1b[31merror\x1b[0m",
                Severity::Warning => "\x1b[33mwarn \x1b[0m",
            };
            println!("{}  {}: {}", tag, i.path.display(), i.message);
        }
        println!();
        println!(
            "dsl-lint: {} file(s) scanned, {} ok, {} error(s), {} warning(s)",
            self.files_scanned, self.files_ok, self.errors, self.warnings
        );
    }
}

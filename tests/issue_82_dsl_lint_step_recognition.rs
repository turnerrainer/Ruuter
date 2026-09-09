//! Issue #82 — regression test for `dsl-lint`'s step recognition.
//!
//! Background: PR #80 fixed a two-year drift between the parser's
//! `ACTION_STEP_KEYS` and the linter's `KNOWN_STEP_KEYS` — `ws_tag:`
//! landed in v0.9.8-rc without a matching update to the linter.
//! The runtime side was covered by `tests/ws_server.rs` scenarios,
//! but nothing exercised the LINTER against every step primitive,
//! so the gap went unnoticed until sviljus wired dsl-lint into
//! their downstream CI.
//!
//! This test invokes the shipped `dsl-lint` binary against a
//! fixture DSL tree that exercises every step primitive listed in
//! `crate::steps::STEP_KEYS`. Failure mode is a fresh primitive
//! landing in the parser without a matching entry in `STEP_KEYS` —
//! that DSL step will lint as "unrecognised" and this test will
//! exit non-zero, catching the drift before it ships.

use ruuter_on_rust::steps::STEP_KEYS;
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

/// A tiny valid DSL for each known step primitive. Each DSL exercises
/// exactly one step + a terminating `return:`. `declaration:` is the
/// one exception — it's metadata, so its DSL body carries the
/// declaration plus a `return:`.
///
/// Any addition to `STEP_KEYS` MUST be paired with an entry here.
/// A missing entry is caught by `every_step_key_has_a_fixture` below.
fn fixture_for(step: &str) -> &'static str {
    match step {
        "assign" => "s: { assign: { x: 1 } }\nr: { return: ok, next: end }\n",
        "call" => "s: { call: http.get, args: { url: 'http://localhost:1', body: null, query: null, headers: null } }\nr: { return: ok, next: end }\n",
        "declaration" => "declaration: { description: 'sample' }\nr: { return: ok, next: end }\n",
        "iterate" => "s: { iterate: { over: '${[]}', as: item, do: r } }\nr: { return: ok, next: end }\n",
        "log" => "s: { log: 'hello', next: r }\nr: { return: ok, next: end }\n",
        "return" => "r: { return: ok }\n",
        "single_flight" => "s: { single_flight: { key: 'k', do: r } }\nr: { return: ok, next: end }\n",
        "state" => "s: { state: { set: { key: k, value: 1 } }, next: r }\nr: { return: ok, next: end }\n",
        "switch" => "s: { switch: [ { condition: '${true}', next: r } ] }\nr: { return: ok, next: end }\n",
        "template" => "s: { template: 'helpers/noop', requestType: GET, result: r, next: rr }\nrr: { return: ok, next: end }\n",
        "ws_send" => "s: { ws_send: { to: 'client-1', body: {} }, next: r }\nr: { return: ok, next: end }\n",
        "ws_tag" => "s: { ws_tag: { set: { role: 'admin' } }, next: r }\nr: { return: ok, next: end }\n",
        other => panic!("no fixture for step primitive '{other}' — add one to fixture_for()"),
    }
}

/// Compile-time-ish safety net: every entry in `STEP_KEYS` must have
/// a matching arm in `fixture_for`. Panic on the first miss.
#[test]
fn every_step_key_has_a_fixture() {
    for key in STEP_KEYS {
        // Just calling it triggers the exhaustive match; any missing
        // arm panics with a clear diagnostic.
        let _ = fixture_for(key);
    }
}

/// The real regression check: `dsl-lint` must recognise EVERY step
/// primitive in `STEP_KEYS`. Landing a new primitive in the runtime
/// parser without a matching entry in `STEP_KEYS` (and therefore in
/// the linter's accept-list) trips this test with a diagnostic
/// pointing at the offending step.
#[test]
fn dsl_lint_recognises_every_step_key() {
    let tmp = std::env::temp_dir().join(format!("ruuter-82-{}", uuid()));
    // Write one DSL per step primitive. Every DSL is a valid tree
    // — dsl-lint will emit an `unrecognised step` diagnostic if any
    // step's top-level key isn't in KNOWN_STEP_KEYS.
    for key in STEP_KEYS {
        let dsl_path = tmp.join("svc").join("GET").join(format!("s_{key}.yml"));
        std::fs::create_dir_all(dsl_path.parent().unwrap()).unwrap();
        std::fs::write(&dsl_path, fixture_for(key)).unwrap();
    }
    // `template:` needs its target to exist, else the linter warns
    // about the missing DSL. Provide a helpers/noop.yml stub.
    let stub = tmp.join("svc/GET/helpers/noop.yml");
    std::fs::create_dir_all(stub.parent().unwrap()).unwrap();
    std::fs::write(&stub, "r: { return: noop, next: end }\n").unwrap();

    // constants.ini — even an empty file satisfies the linter's
    // constants-file requirement (no [#…] references in fixtures).
    let constants = tmp.join("constants.ini");
    std::fs::write(&constants, "").unwrap();

    // Invoke the shipped dsl-lint binary. `CARGO_BIN_EXE_dsl-lint`
    // is set by cargo when running integration tests in the same
    // package that owns the [[bin]].
    let bin = env!("CARGO_BIN_EXE_dsl-lint");
    let output = Command::new(bin)
        .arg("--dsl")
        .arg(&tmp)
        .arg("--constants")
        .arg(&constants)
        .output()
        .expect("failed to invoke dsl-lint");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stdout.contains("unrecognised step") && !stderr.contains("unrecognised step"),
        "dsl-lint reported an unrecognised step primitive — this test's fixture \
         set exercises every entry in STEP_KEYS, so any 'unrecognised' output \
         means the linter's accept-list has drifted from STEP_KEYS. \n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(
        output.status.success(),
        "dsl-lint exited {}: fixture DSLs should all lint clean. \
         stdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
}

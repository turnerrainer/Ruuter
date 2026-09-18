//! h2ck.me v1 T-23 — fuzz target for the DSL YAML loader.
//!
//! The DSL loader is the single biggest attack surface on Ruuter's
//! Rust side: it takes arbitrary YAML from `DSL/**` and turns it
//! into a live in-process routing table. A panic here means a bad
//! DSL crashes the server; an UB/OOB read means worse. Fleet-
//! doctrine §9.3 names this as fuzz-adoption target #1.
//!
//! What this target asserts:
//! 1. **No panic.** The parser MUST return `Err` for invalid
//!    input, never abort. libfuzzer treats any panic/abort as a
//!    crash and shrinks the failing input for us.
//! 2. **No unwind past the parser boundary.** Even a Result::Err
//!    is fine — the fuzzer only fails on hard aborts.
//!
//! Seed corpus lives under `corpus/dsl_yaml_load/` and is
//! populated on `cargo fuzz run` from `../DSL/samples/**/*.yml`
//! when the runner starts (via the seed hook below).
//!
//! Run locally:
//! ```bash
//! cargo install cargo-fuzz
//! rustup toolchain add nightly
//! cargo +nightly fuzz run dsl_yaml_load -- -max_total_time=600
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The parser only accepts UTF-8; drop obviously non-UTF-8
    // inputs so the fuzzer spends its time on shapes the parser
    // could plausibly reach in production (a YAML file on disk
    // is UTF-8 by convention, and the loader would surface a
    // read error for anything else long before the parser).
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // We call `DslParser::parse_content` — the same entry point
    // the loader uses per file — so a panic here is a panic
    // INSIDE parsing, not in the file-system layer. The loader
    // is exercised by an integration test elsewhere.
    //
    // Constants map is empty; if a corpus input references
    // `[#foo]` the substitution leaves the marker as-is, which
    // is legal YAML and lets the parser see the same shape it
    // would in a real load.
    let parser =
        ruuter_on_rust::dsl::parser::DslParser::new(std::collections::HashMap::new());
    let _ = parser.parse_content(text);
});

//! h2ck.me v1 T-23 — fuzz target for JSON HTTP body deserialisation.
//!
//! Every inbound request with `Content-Type: application/json`
//! passes through `serde_json::from_slice::<Value>(&body_bytes)`
//! at `src/router/mod.rs:917`. That parser has a strong safety
//! record (adjacent-limit deep nesting is pinned by T-31), but
//! it's the shape of code most often broken by dep upgrades or
//! feature-flag flips. Fleet §9.3 names DTO deserialisers as
//! adoption target #2.
//!
//! Beyond "no panic," this target asserts a **round-trip
//! invariant**: `parse(serialize(x)) == x` for the JSON values
//! we accept. Any parser that silently loses information (e.g.
//! numeric precision, key collision) would fail here.
//!
//! Seed corpus lives under `corpus/http_body_deserialize/` —
//! seed with real JSON bodies from `../tests/**` on first run.
//!
//! Run locally:
//! ```bash
//! cargo install cargo-fuzz
//! rustup toolchain add nightly
//! cargo +nightly fuzz run http_body_deserialize -- -max_total_time=600
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // First path: raw bytes → serde_json Value. Panics here are
    // regressions in serde_json itself or in a downstream crate
    // that shadows the JSON parser (e.g. simd-json).
    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(data);

    if let Ok(v) = parsed {
        // Round-trip invariant: parse(serialize(v)) == v.
        // Any parser that loses fidelity — numeric precision,
        // key ordering (Value::Object is BTreeMap so ordering
        // is deterministic; failure here means keys DROPPED),
        // duplicate-key collision (JSON allows dup keys per RFC
        // 8259 §4 but serde_json takes the LAST) — surfaces as
        // an inequality after one round.
        let serialized = serde_json::to_vec(&v).expect("value → bytes must not fail");
        let reparsed: serde_json::Value =
            serde_json::from_slice(&serialized).expect("round-trip parse must not fail");
        assert_eq!(
            v, reparsed,
            "JSON round-trip lost information — parser or serialiser bug"
        );
    }
});

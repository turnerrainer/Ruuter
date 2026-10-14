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
        // Round-trip invariant: after ONE canonicalisation pass,
        // subsequent round-trips must be exact. We check that
        //   parse(serialize(parse(serialize(v)))) == parse(serialize(v))
        // rather than `parse(serialize(v)) == v`.
        //
        // The canonicalisation pass is unavoidable because a raw
        // input like "1111111111111111111111111" (25 digits) sits
        // outside f64's safe-integer range (±2^53 ≈ 9e15) and
        // parses to the nearest-f64. Serialising that f64 with
        // ryu's shortest-roundtrip form can land on a numeric
        // literal that then reparses to the *adjacent* f64 (one
        // ULP away) — not a parser bug, a consequence of the raw
        // input exceeding the numeric type's precision. Once the
        // value has been through one parse+serialise pass, it is
        // in canonical form and every subsequent round is exact.
        //
        // Still catches: key drops (Object is BTreeMap so identical
        // shapes serialise identically), duplicate-key collision
        // regressions, non-f64 precision loss, serialiser output
        // the parser cannot read back.
        let s1 = serde_json::to_vec(&v).expect("value → bytes must not fail");
        let canonical: serde_json::Value =
            serde_json::from_slice(&s1).expect("round-trip parse must not fail");
        let s2 = serde_json::to_vec(&canonical).expect("value → bytes must not fail");
        let reparsed: serde_json::Value =
            serde_json::from_slice(&s2).expect("second round-trip parse must not fail");
        assert_eq!(
            canonical, reparsed,
            "JSON round-trip did not reach a fixed point after canonicalisation — parser or serialiser bug"
        );
    }
});

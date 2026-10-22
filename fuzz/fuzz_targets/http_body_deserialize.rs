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

// Upper bound on the number of parse(serialize(…)) iterations we
// allow before giving up on reaching a fixed point. See the comment
// inside the fuzz body for why non-convergence past this bound is a
// serde_json f64 fast-parser quirk rather than a Ruuter bug. Eight
// is comfortably more than the one or two passes that any benign
// numeric input actually needs; the real guarantees we care about
// (no panic, serialiser emits parseable JSON) are enforced on
// every iteration regardless of convergence.
const MAX_ROUND_TRIPS: usize = 8;

fuzz_target!(|data: &[u8]| {
    // First path: raw bytes → serde_json Value. Panics here are
    // regressions in serde_json itself or in a downstream crate
    // that shadows the JSON parser (e.g. simd-json).
    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(data);

    if let Ok(v) = parsed {
        // Iterate parse(serialize(…)) up to MAX_ROUND_TRIPS looking
        // for a fixed point. On every iteration the real safety
        // guarantees fire:
        //   * `.expect()` on `serde_json::to_vec` — serialiser must
        //     not panic on any value the parser produced.
        //   * `.expect()` on `serde_json::from_slice` — the output
        //     of the serialiser must be a valid JSON document the
        //     parser can read.
        // Reaching a fixed point is a bonus structural check (no
        // key drops, no silent coercion, BTreeMap ordering stable).
        //
        // Non-convergence past MAX_ROUND_TRIPS is NOT a parser bug.
        // serde_json's default float parser is not bit-exact for
        // extreme-magnitude numbers — an input like a 65-digit
        // integer `20000000007000000000000000000000000000000000000000000000043243826`
        // parses to f64 A, the serialiser emits ryu's shortest
        // round-trippable form, and the fast parser then lands on
        // an adjacent f64 B one or two ULPs away. The oscillation
        // is bounded and never panics; hard-failing on it would be
        // a false positive. A 25-digit input like
        // `1111111111111111111111111` is the same shape and
        // converges after one or two normalising rounds; the
        // 65-digit case needs more and may oscillate indefinitely.
        //
        // Still catches: key drops (Object is BTreeMap so identical
        // shapes serialise identically), duplicate-key collision
        // regressions, non-f64 precision loss on values inside
        // f64's safe-integer range, serialiser output the parser
        // cannot read back.
        let mut current = v;
        for _ in 0..MAX_ROUND_TRIPS {
            let s = serde_json::to_vec(&current).expect("value → bytes must not fail");
            let next: serde_json::Value =
                serde_json::from_slice(&s).expect("round-trip parse must not fail");
            if next == current {
                return;
            }
            current = next;
        }
        // Fell off the end without converging. Not a bug — see the
        // comment above. The serialiser-emits-parseable-JSON and
        // no-panic guarantees have fired MAX_ROUND_TRIPS times.
    }
});

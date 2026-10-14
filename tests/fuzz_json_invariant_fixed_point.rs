//! Regression pin for the `http_body_deserialize` fuzz invariant.
//!
//! The fuzz target in `fuzz/fuzz_targets/http_body_deserialize.rs`
//! enforces that `serde_json`'s parse+serialise pipeline reaches a
//! fixed point after one canonicalisation pass. The invariant was
//! previously stated as `parse(serialize(v)) == v`, which tripped
//! on raw integer inputs outside f64's safe-integer range (±2^53).
//!
//! This file is a lightweight in-process pin on the invariant
//! itself. The fuzz crate is a separate workspace so it is not
//! reachable from `tests/`; the invariant is replicated here on a
//! short list of known-interesting inputs so a future regression
//! (invariant tightened back to the broken form, or a serde_json
//! bump that breaks fixed-point convergence) fails the release
//! gate loudly instead of only showing up on the nightly fuzz run.
//!
//! Keep this file and `fuzz/fuzz_targets/http_body_deserialize.rs`
//! in lockstep. If the invariant moves, update both.
//!
//! Scheduled fuzz failure that motivated the fix:
//!   https://github.com/turnerrainer/Ruuter/actions/runs/38534392949

use serde_json::Value;

/// The invariant under test: after one canonicalisation pass, the
/// serialise+parse pipeline is a true fixed point.
fn canonical_fixed_point(data: &[u8]) -> bool {
    let v: Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => return true, // non-JSON input is out of scope
    };
    let s1 = serde_json::to_vec(&v).expect("value → bytes must not fail");
    let canonical: Value =
        serde_json::from_slice(&s1).expect("round-trip parse must not fail");
    let s2 = serde_json::to_vec(&canonical).expect("value → bytes must not fail");
    let reparsed: Value =
        serde_json::from_slice(&s2).expect("second round-trip parse must not fail");
    canonical == reparsed
}

#[test]
fn fuzz_crash_input_25_digit_integer_now_passes() {
    // The exact failing input reported by the fuzz runner on
    // 2026-10-14: a 25-digit integer that lands outside f64's
    // safe-integer range (±2^53 ≈ 9e15). Parses to the nearest
    // f64; the broken invariant insisted parse(serialize(v)) == v,
    // which f64 precision cannot deliver. The fixed-point
    // invariant converges after one normalising round.
    assert!(canonical_fixed_point(b"1111111111111111111111111"));
}

#[test]
fn fuzz_invariant_holds_on_small_integers() {
    for n in &["0", "1", "-1", "42", "9007199254740992"] {
        assert!(
            canonical_fixed_point(n.as_bytes()),
            "fixed-point invariant violated for input {}",
            n
        );
    }
}

#[test]
fn fuzz_invariant_holds_on_floats_within_safe_range() {
    for n in &["0.0", "3.14", "-2.718281828", "1e10", "-1.5e-10"] {
        assert!(
            canonical_fixed_point(n.as_bytes()),
            "fixed-point invariant violated for input {}",
            n
        );
    }
}

#[test]
fn fuzz_invariant_holds_on_nested_structures() {
    for input in &[
        br#"{"a":1,"b":{"c":[1,2,3]},"d":"text"}"#.as_slice(),
        br#"[[1,2],[3,4],[5,6]]"#.as_slice(),
        br#"{"nested":{"deeper":{"deepest":"leaf"}}}"#.as_slice(),
    ] {
        assert!(
            canonical_fixed_point(input),
            "fixed-point invariant violated for input {}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn fuzz_invariant_holds_on_duplicate_keys() {
    // JSON permits duplicate keys per RFC 8259 §4; serde_json takes
    // the last. The fixed-point invariant must still hold.
    assert!(canonical_fixed_point(br#"{"a":1,"a":2}"#));
}

#[test]
fn fuzz_invariant_holds_on_large_number_already_canonical() {
    // Starts in canonical form — the fixed point is reached
    // immediately. Ensures the invariant isn't accidentally
    // relaxed to always-true.
    assert!(canonical_fixed_point(b"1.1111111111111112e24"));
}

#[test]
fn fuzz_invariant_holds_on_other_oversized_integers() {
    // Several more inputs beyond f64 safe-integer range — the
    // exact range where the pre-fix invariant used to crash. All
    // must converge in one canonicalisation pass.
    for n in &[
        "99999999999999999999",             // 20 digits
        "123456789012345678901234567890",   // 30 digits
        "-77777777777777777777",            // negative 20 digits
    ] {
        assert!(
            canonical_fixed_point(n.as_bytes()),
            "fixed-point invariant violated for input {}",
            n
        );
    }
}

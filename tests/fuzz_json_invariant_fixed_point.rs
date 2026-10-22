//! Regression pin for the `http_body_deserialize` fuzz invariant.
//!
//! The fuzz target in `fuzz/fuzz_targets/http_body_deserialize.rs`
//! enforces that `serde_json`'s parse+serialise pipeline reaches a
//! fixed point within a bounded number of iterations. The invariant
//! was previously stated as `parse(serialize(v)) == v`, which tripped
//! on raw integer inputs outside f64's safe-integer range (±2^53);
//! and then tightened to "one canonicalisation pass → fixed point,"
//! which held for a 25-digit input but still broke for a 65-digit
//! input where serde_json's fast float parser lands on an adjacent
//! f64 on every reparse (one or two ULP oscillation).
//!
//! This file is a lightweight in-process pin on the invariant
//! itself. The fuzz crate is a separate workspace so it is not
//! reachable from `tests/`; the invariant is replicated here on a
//! short list of known-interesting inputs so a future regression
//! (invariant tightened back to a broken form, serialiser emits
//! invalid JSON, round-trip panic) fails the release gate loudly
//! instead of only showing up on the nightly fuzz run.
//!
//! Keep this file and `fuzz/fuzz_targets/http_body_deserialize.rs`
//! in lockstep. If the invariant moves, update both.
//!
//! Scheduled fuzz failures that motivated the fix:
//!   https://github.com/turnerrainer/Ruuter/actions/runs/38534392949
//!   https://github.com/turnerrainer/Ruuter/actions/runs/39567669427

use serde_json::Value;

/// The invariant under test: after one canonicalisation pass, the
/// serialise+parse pipeline is a true fixed point.
fn canonical_fixed_point(data: &[u8]) -> bool {
    let v: Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => return true, // non-JSON input is out of scope
    };
    let s1 = serde_json::to_vec(&v).expect("value → bytes must not fail");
    let canonical: Value = serde_json::from_slice(&s1).expect("round-trip parse must not fail");
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
        "99999999999999999999",           // 20 digits
        "123456789012345678901234567890", // 30 digits
        "-77777777777777777777",          // negative 20 digits
    ] {
        assert!(
            canonical_fixed_point(n.as_bytes()),
            "fixed-point invariant violated for input {}",
            n
        );
    }
}

/// Mirror of the actual fuzz target loop: iterate parse(serialize(…))
/// up to `MAX_ROUND_TRIPS` passes and return `true` if we either
/// converge or run out of iterations without panicking. The guarantees
/// we care about — serialiser does not panic, serialised output is
/// parseable — fire on every iteration. Non-convergence is tolerated
/// (serde_json fast-parser quirk at extreme magnitudes).
fn bounded_round_trip_no_panic(data: &[u8]) -> bool {
    const MAX_ROUND_TRIPS: usize = 8;
    let v: Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => return true,
    };
    let mut current = v;
    for _ in 0..MAX_ROUND_TRIPS {
        let s = serde_json::to_vec(&current).expect("value → bytes must not fail");
        let next: Value =
            serde_json::from_slice(&s).expect("round-trip parse must not fail");
        if next == current {
            return true;
        }
        current = next;
    }
    // Non-convergence is accepted — see module doc for why.
    true
}

#[test]
fn fuzz_crash_input_65_digit_integer_oscillates_but_does_not_panic() {
    // The crash input reported by the fuzz runner on 2026-10-21:
    // a 65-digit integer that parses to f64 `2.0000000007000003e+64`.
    // serde_json's fast float parser lands on `2.0000000007000006e+64`
    // on reparse (adjacent f64, one or two ULPs away), and the
    // oscillation does not resolve within one canonicalisation pass.
    // The guarantee we care about: the pipeline does not panic and
    // the serialiser always emits parseable JSON.
    let input = b"20000000007000000000000000000000000000000000000000000000043243826";
    assert!(bounded_round_trip_no_panic(input));

    // Pin that this input is NOT one-pass-convergent — if a future
    // serde_json release fixes the fast parser so this converges in
    // one pass, we'll see this assertion flip and can tighten the
    // fuzz invariant back if desired.
    assert!(
        !canonical_fixed_point(input),
        "if serde_json's default float parser becomes bit-exact at \
         extreme magnitudes, revisit the fuzz target's MAX_ROUND_TRIPS \
         loop and consider tightening the invariant back to a single \
         canonicalisation pass"
    );
}

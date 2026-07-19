#!/usr/bin/env python3
"""Automated tests for bench/compare.py.

Runs the comparator as a subprocess with crafted baseline+current
files, asserts on exit code and output. Because compare.py's whole
job is "fail loudly on regression," these tests specifically try to
BREAK it — the failure modes we care about are silent passes on real
regressions and false alarms on within-tolerance runs.

Run: `python3 -m unittest bench/tests/test_compare.py`
     or `python3 bench/tests/test_compare.py`
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMPARE = os.path.abspath(os.path.join(HERE, "..", "compare.py"))


def write(path, data):
    with open(path, "w") as f:
        json.dump(data, f)


def run(baseline, current, *extra):
    """Invoke compare.py. Returns (exit_code, stdout, stderr)."""
    with tempfile.TemporaryDirectory() as td:
        bp = os.path.join(td, "baseline.json")
        cp = os.path.join(td, "current.json")
        write(bp, baseline)
        write(cp, current)
        res = subprocess.run(
            [sys.executable, COMPARE, "--baseline", bp, "--current", cp, *extra],
            capture_output=True, text=True,
        )
        return res.returncode, res.stdout, res.stderr


def scen(rps, p50=1.0):
    return {"rps": rps, "p50_ms": p50, "p75_ms": p50 * 1.5, "p90_ms": p50 * 2, "p99_ms": p50 * 5}


class TestExitCodes(unittest.TestCase):
    def test_identical_baseline_passes(self):
        base = {"scenarios": {"a": scen(1000), "b": scen(2000)}}
        code, out, _ = run(base, base)
        self.assertEqual(code, 0, msg=out)
        self.assertIn("within tolerance", out)

    def test_within_15pct_passes_at_default_tolerance(self):
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(870)}}  # -13%, within default 15%
        code, _, _ = run(base, curr)
        self.assertEqual(code, 0)

    def test_over_15pct_fails_at_default_tolerance(self):
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(800)}}  # -20%, exceeds default
        code, out, _ = run(base, curr)
        self.assertEqual(code, 1)
        self.assertIn("regressed", out)
        self.assertIn("a:", out)

    def test_custom_rps_tolerance(self):
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(800)}}  # -20%
        # Passes when tolerance is 25%
        code, _, _ = run(base, curr, "--rps-tolerance", "0.25")
        self.assertEqual(code, 0)
        # Fails when tolerance is 10%
        code, _, _ = run(base, curr, "--rps-tolerance", "0.10")
        self.assertEqual(code, 1)


class TestMissingScenarios(unittest.TestCase):
    """A regression that silently disappears from the current run
    must NOT hide behind a green comparator. This is the exact bug
    class 'audit finds bugs in prior round's fixes' warns about."""

    def test_missing_scenario_in_current_fails(self):
        base = {"scenarios": {"a": scen(1000), "b": scen(2000)}}
        curr = {"scenarios": {"a": scen(1000)}}  # b missing
        code, out, _ = run(base, curr)
        self.assertEqual(code, 1, msg=out)
        self.assertIn("MISSING", out)
        self.assertIn("b:", out)

    def test_extra_scenario_in_current_is_ignored(self):
        # Extra scenarios in current (e.g. added in a PR before baseline
        # refresh) are fine — they just don't count toward gate.
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(1000), "b": scen(500)}}
        code, _, _ = run(base, curr)
        self.assertEqual(code, 0)


class TestLatencyGate(unittest.TestCase):
    def test_lat_regression_fails_by_default(self):
        base = {"scenarios": {"a": scen(1000, p50=1.0)}}
        curr = {"scenarios": {"a": scen(1000, p50=2.0)}}  # +100% p50, rps unchanged
        code, out, _ = run(base, curr)
        self.assertEqual(code, 1)
        self.assertIn("p50 regressed", out)

    def test_lat_regression_warn_only_passes(self):
        base = {"scenarios": {"a": scen(1000, p50=1.0)}}
        curr = {"scenarios": {"a": scen(1000, p50=2.0)}}
        code, out, _ = run(base, curr, "--lat-warn-only")
        self.assertEqual(code, 0)
        self.assertIn("WARNINGS", out)
        self.assertIn("p50 regressed", out)

    def test_lat_within_tolerance_no_warning(self):
        base = {"scenarios": {"a": scen(1000, p50=1.0)}}
        curr = {"scenarios": {"a": scen(1000, p50=1.20)}}  # +20%, within 25% default
        code, out, _ = run(base, curr, "--lat-warn-only")
        self.assertEqual(code, 0)
        self.assertNotIn("p50 regressed", out)

    def test_rps_and_lat_both_regressed_reports_both(self):
        base = {"scenarios": {"a": scen(1000, p50=1.0)}}
        curr = {"scenarios": {"a": scen(500, p50=3.0)}}
        code, out, _ = run(base, curr)
        self.assertEqual(code, 1)
        # Both failures must be surfaced — not just the first
        self.assertIn("rps regressed", out)
        self.assertIn("p50 regressed", out)


class TestErrorHandling(unittest.TestCase):
    def test_missing_baseline_file_exits_2(self):
        with tempfile.TemporaryDirectory() as td:
            cp = os.path.join(td, "current.json")
            write(cp, {"scenarios": {}})
            res = subprocess.run(
                [sys.executable, COMPARE, "--baseline", "/nonexistent/x.json", "--current", cp],
                capture_output=True, text=True,
            )
            self.assertEqual(res.returncode, 2)

    def test_empty_baseline_scenarios_exits_2(self):
        code, _, err = run({"scenarios": {}}, {"scenarios": {"a": scen(1)}})
        self.assertEqual(code, 2)
        self.assertIn("baseline has no scenarios", err)

    def test_malformed_baseline_exits_2(self):
        with tempfile.TemporaryDirectory() as td:
            bp = os.path.join(td, "baseline.json")
            cp = os.path.join(td, "current.json")
            with open(bp, "w") as f:
                f.write("not json")
            write(cp, {"scenarios": {"a": scen(1)}})
            res = subprocess.run(
                [sys.executable, COMPARE, "--baseline", bp, "--current", cp],
                capture_output=True, text=True,
            )
            self.assertEqual(res.returncode, 2)


class TestBoundaryConditions(unittest.TestCase):
    def test_exact_tolerance_boundary_passes(self):
        # -14.999% just barely within 15% tolerance
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(850.1)}}
        code, _, _ = run(base, curr)
        self.assertEqual(code, 0)

    def test_just_over_tolerance_fails(self):
        # -15.001% fails
        base = {"scenarios": {"a": scen(1000)}}
        curr = {"scenarios": {"a": scen(849.9)}}
        code, _, _ = run(base, curr)
        self.assertEqual(code, 1)

    def test_zero_baseline_rps_does_not_crash(self):
        # Pathological but plausible — if the baseline was captured
        # while broken, don't panic. Just treat delta as 0 (comparison
        # meaningless, but we shouldn't zerodiv-crash the CI).
        base = {"scenarios": {"a": {"rps": 0, "p50_ms": 0}}}
        curr = {"scenarios": {"a": {"rps": 1000, "p50_ms": 1.0}}}
        code, _, _ = run(base, curr)
        self.assertEqual(code, 0)

    def test_improvement_does_not_fail(self):
        # 2× throughput is not a regression, even if p50 changed
        base = {"scenarios": {"a": scen(1000, p50=5.0)}}
        curr = {"scenarios": {"a": scen(2000, p50=1.0)}}
        code, _, _ = run(base, curr)
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()

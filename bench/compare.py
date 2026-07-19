#!/usr/bin/env python3
"""bench/compare.py — diff a fresh results.json against bench/baseline.json.

Exit 0 if every scenario is within the configured tolerance.
Exit 1 if any scenario regresses more than --rps-tolerance below baseline
       OR any p50 latency exceeds baseline p50 by more than --lat-tolerance.
Exit 2 on I/O or schema error.

Usage:
    bench/compare.py --baseline bench/baseline.json --current bench/results.json
    bench/compare.py --baseline bench/baseline.json --current bench/results.json \
                     --rps-tolerance 0.15 --lat-tolerance 0.20

Latency comparison is warn-only in CI mode (--lat-warn-only) to reduce
false positives from GitHub-hosted runner noise on tail percentiles.
Throughput regression is the hard gate — it's the number that matters.
"""
import argparse
import json
import sys


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        print(f"error: cannot read {path}: {e}", file=sys.stderr)
        sys.exit(2)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", required=True)
    ap.add_argument("--current", required=True)
    ap.add_argument(
        "--rps-tolerance", type=float, default=0.15,
        help="max fraction below baseline rps before failure (default 0.15 = 15%%)"
    )
    ap.add_argument(
        "--lat-tolerance", type=float, default=0.25,
        help="max fraction above baseline p50 before latency warning (default 0.25 = 25%%)"
    )
    ap.add_argument(
        "--lat-warn-only", action="store_true",
        help="latency regressions warn but don't fail (recommended for GH-hosted CI)"
    )
    args = ap.parse_args()

    base = load(args.baseline)
    curr = load(args.current)

    base_scen = base.get("scenarios", {})
    curr_scen = curr.get("scenarios", {})

    if not base_scen:
        print("error: baseline has no scenarios", file=sys.stderr)
        sys.exit(2)

    failures = []
    warnings = []
    rows = []

    for name in sorted(base_scen):
        b = base_scen[name]
        c = curr_scen.get(name)
        if c is None:
            failures.append(f"{name}: MISSING in current run")
            rows.append((name, "?", f"{b['rps']:.0f}", "?", "MISSING"))
            continue

        b_rps = b["rps"]
        c_rps = c["rps"]
        rps_delta = (c_rps - b_rps) / b_rps if b_rps > 0 else 0.0

        b_p50 = b.get("p50_ms", 0)
        c_p50 = c.get("p50_ms", 0)
        lat_delta = (c_p50 - b_p50) / b_p50 if b_p50 > 0 else 0.0

        status = "ok"
        if rps_delta < -args.rps_tolerance:
            failures.append(
                f"{name}: rps regressed {rps_delta*100:+.1f}% "
                f"(baseline={b_rps:.0f}, current={c_rps:.0f}, "
                f"tolerance=-{args.rps_tolerance*100:.0f}%)"
            )
            status = "FAIL rps"
        if lat_delta > args.lat_tolerance:
            msg = (
                f"{name}: p50 regressed {lat_delta*100:+.1f}% "
                f"(baseline={b_p50}ms, current={c_p50}ms, "
                f"tolerance=+{args.lat_tolerance*100:.0f}%)"
            )
            if args.lat_warn_only:
                warnings.append(msg)
                if status == "ok":
                    status = "warn lat"
            else:
                failures.append(msg)
                status = "FAIL lat" if status == "ok" else status + "+lat"

        rows.append((name, f"{c_rps:.0f}", f"{b_rps:.0f}", f"{rps_delta*100:+.1f}%", status))

    # Report
    header = f"{'scenario':<22} {'current rps':>12} {'baseline rps':>13} {'delta':>8}  status"
    print(header)
    print("-" * len(header))
    for r in rows:
        print(f"{r[0]:<22} {r[1]:>12} {r[2]:>13} {r[3]:>8}  {r[4]}")

    print()
    if warnings:
        print("WARNINGS:")
        for w in warnings:
            print(f"  {w}")
        print()
    if failures:
        print("FAILURES:")
        for f in failures:
            print(f"  {f}")
        sys.exit(1)

    print("all scenarios within tolerance")
    sys.exit(0)


if __name__ == "__main__":
    main()

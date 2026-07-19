#!/usr/bin/env bash
# bench/refresh-baseline.sh — capture a fresh baseline after an
# intentional perf change.
#
# Runs the bench suite 3 times, keeps the MEDIAN rps per scenario
# (reduces per-run jitter), writes bench/baseline.json.
#
# Usage:
#   bench/refresh-baseline.sh
#   bench/refresh-baseline.sh --runs 5     # more runs, less noise

set -eu

RUNS=3
PORT=8080
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --runs)       RUNS="$2"; shift 2 ;;
    --port)       PORT="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

RUN_ARGS=(--port "$PORT")
[ "$SKIP_BUILD" -eq 1 ] && RUN_ARGS+=(--skip-build)

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

TMPDIR="$(mktemp -d -t ruuter-baseline.XXXXXX)"
trap 'rm -rf "$TMPDIR"' EXIT

echo ">> capturing $RUNS runs"
for i in $(seq 1 "$RUNS"); do
  echo ">>> run $i / $RUNS"
  bench/run.sh "${RUN_ARGS[@]}" --output "$TMPDIR/run-$i.json"
done

# Median across runs, per scenario, per metric
python3 - "$TMPDIR" "$RUNS" > bench/baseline.json <<'PY'
import json, sys, os, statistics
tmpdir, runs = sys.argv[1], int(sys.argv[2])
runs_data = [json.load(open(f"{tmpdir}/run-{i}.json")) for i in range(1, runs + 1)]
scen_names = set()
for r in runs_data:
    scen_names.update(r.get("scenarios", {}).keys())
merged = {}
for name in sorted(scen_names):
    per_metric = {"rps": [], "p50_ms": [], "p75_ms": [], "p90_ms": [], "p99_ms": []}
    for r in runs_data:
        s = r["scenarios"].get(name)
        if not s: continue
        for k in per_metric:
            if k in s: per_metric[k].append(s[k])
    merged[name] = {k: round(statistics.median(v), 3) for k, v in per_metric.items() if v}
meta = runs_data[-1]
out = {
    "captured_at": meta.get("captured_at"),
    "git_sha": meta.get("git_sha"),
    "version": meta.get("version"),
    "kernel": meta.get("kernel"),
    "cpus": meta.get("cpus"),
    "runs_median": runs,
    "note": "regenerate via bench/refresh-baseline.sh after intentional perf changes",
    "scenarios": merged,
}
print(json.dumps(out, indent=2))
PY

echo ">> wrote bench/baseline.json (median of $RUNS runs)"

#!/usr/bin/env bash
# A/B perf harness for tasks 042 / 043 / 044.
#
# For each of the three v0.6.0 features that changed the request-hot
# path, this runs the SAME workload with the feature enabled and
# disabled, N times each, and prints all runs so a downstream script
# can pick the median.
#
# 042 (single_flight): identical workload under duplicate-request
#     load, one DSL wraps its body in single_flight and the other
#     doesn't.
# 043 (UDS transport):  identical workload, one DSL calls the target
#     via a UDS alias, the other via TCP loopback to the same
#     second-instance ruuter.
# 044 (self-call short-circuit): identical DSL calling itself; the
#     "without" case sets RUUTER_DISABLE_SELF_CALL_SHORTCIRCUIT=true
#     so the caller round-trips through reqwest + TCP loopback.
#
# ## Runtime environment
#
# Localhost benching on a developer laptop is noisy — other
# processes, docker containers, browsers, thermal state and OS
# scheduling all affect the numbers. For headline-grade results,
# use an isolated single-tenant host (dedicated EC2, bare-metal
# runner, or a compose-isolated CI executor). See
# `bench/README.md` for the AWS instance-type + kernel-tuning
# checklist.

set -eu
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

RUNS="${RUNS:-3}"
OUT_DIR="${OUT_DIR:-/tmp}"

# Nuke any stragglers (host processes only — leave docker /app/ ones)
for pid in $(pgrep -f "target/release/ruuter-on-rust"); do
  kill -9 "$pid" >/dev/null 2>&1 || true
done
sleep 1

echo "############################################################"
echo "# A/B perf harness — runs=$RUNS, output=$OUT_DIR"
echo "# host=$(uname -srm)  cpus=$(nproc)  bench-git=$(git rev-parse --short HEAD)"
echo "############################################################"

echo
echo "### 044 A/B — self-call shortcut vs TCP loopback ###"
for RUN in $(seq 1 "$RUNS"); do
  echo "--- run $RUN ---"
  bench/run.sh --port 8081 --skip-build --output "$OUT_DIR/ab-044-with-$RUN.json" 2>&1 \
    | grep -E "^\s+self-call\s+rps" || echo "  (with-shortcut) no output"
  RUUTER_DISABLE_SELF_CALL_SHORTCIRCUIT=true \
    bench/run.sh --port 8081 --skip-build --output "$OUT_DIR/ab-044-without-$RUN.json" 2>&1 \
    | grep -E "^\s+self-call\s+rps" || echo "  (without-shortcut) no output"
done

echo
echo "### 042 A/B — single_flight vs naive ###"
for RUN in $(seq 1 "$RUNS"); do
  echo "--- run $RUN ---"
  bench/run.sh --port 8081 --skip-build --output "$OUT_DIR/ab-042-$RUN.json" 2>&1 \
    | grep -E "^\s+042-"
done

echo
echo "### 043 A/B — UDS via alias vs TCP loopback (cross-instance) ###"
for RUN in $(seq 1 "$RUNS"); do
  echo "--- run $RUN ---"
  bench/run-043-ab.sh 2>&1 | grep -E "^\s+/samples/bench-"
done

echo
echo "############################################################"
echo "# Done — pick medians per row and produce your report."
echo "############################################################"

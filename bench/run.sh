#!/usr/bin/env bash
# bench/run.sh — orchestrate the perf benchmark suite.
#
# Usage:
#   bench/run.sh                    # build, boot, run all scenarios, write bench/results.json
#   bench/run.sh --output <path>    # custom output path
#   bench/run.sh --port <port>      # server port (default 8080)
#   bench/run.sh --skip-build       # assume ./target/release/ruuter-on-rust already built
#
# Prereqs: cargo, wrk, jq, yq (python3 with yaml module OK too).

set -eu

OUTPUT="bench/results.json"
PORT=8080
SKIP_BUILD=0

while [ $# -gt 0 ]; do
  case "$1" in
    --output) OUTPUT="$2"; shift 2 ;;
    --port)   PORT="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing: $1" >&2; exit 3; }; }
need cargo
need wrk
need jq
need python3

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [ "$SKIP_BUILD" -eq 0 ]; then
  echo ">> building release binary"
  cargo build --release --bin ruuter-on-rust >&2
fi

BIN="$REPO_ROOT/target/release/ruuter-on-rust"
if [ ! -x "$BIN" ]; then
  echo "missing binary: $BIN" >&2
  exit 4
fi

if ss -ltn 2>/dev/null | grep -q ":$PORT "; then
  echo "port $PORT already in use — pass --port <free-port> or stop the incumbent process" >&2
  exit 5
fi

LOG="$(mktemp -t ruuter-bench.XXXXXX.log)"
CONFIG="$(mktemp -t ruuter-bench.XXXXXX.yaml)"
cat > "$CONFIG" <<EOF
port: $PORT
config_path: ./DSL
EOF
trap 'kill -9 $SRV_PID >/dev/null 2>&1 || true; rm -f "$CONFIG"' EXIT

echo ">> starting server on port $PORT (log: $LOG, config: $CONFIG)"
RUUTER_ADMIN_ENABLED=false "$BIN" --config "$CONFIG" --dsl DSL --constants constants.ini > "$LOG" 2>&1 &
SRV_PID=$!

# Wait up to 10 s for the server to bind
for _ in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:$PORT/health" > /dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
if ! curl -sf "http://127.0.0.1:$PORT/health" > /dev/null 2>&1; then
  echo "server failed to start; log:" >&2
  tail "$LOG" >&2
  exit 6
fi
echo ">> server up on port $PORT"

# Warm-up: 5 s at low load smooths out first-hit JIT-ish costs
wrk -t2 -c10 -d5s "http://127.0.0.1:$PORT/health" > /dev/null 2>&1

echo ">> running scenarios"

RESULTS_TMP="$(mktemp -t ruuter-bench-results.XXXXXX.json)"
echo '{"scenarios": {}}' > "$RESULTS_TMP"

for SCENARIO_FILE in bench/scenarios/*.bench.yml; do
  # Extract scalar fields, then read header k:v pairs on separate
  # lines. Splitting scalar vars from the header list dodges the
  # nested-quoting trap that broke the first pass (a single-string
  # HDR_ARGS becomes one shell token, not many, and wrk sees the
  # header value fragments as URL arguments).
  SCEN="$(python3 - "$SCENARIO_FILE" <<'PY'
import sys, yaml
with open(sys.argv[1]) as f:
    s = yaml.safe_load(f)
wrk_cfg = s.get("wrk", {})
print(f"NAME={s['name']}")
print(f"ROUTE={s['route']}")
print(f"METHOD={s.get('method', 'GET').upper()}")
print(f"THREADS={wrk_cfg.get('threads', 4)}")
print(f"CONNS={wrk_cfg.get('connections', 64)}")
print(f"DUR={wrk_cfg.get('duration', '10s')}")
for k, v in (s.get("headers") or {}).items():
    # No shell metachars survive here — the reader pulls the
    # unquoted rest-of-line as a single header value string.
    print(f"HDR={k}: {v}")
PY
)"

  NAME=""; ROUTE=""; METHOD=""; THREADS=""; CONNS=""; DUR=""
  HDR_ARGS=()
  while IFS= read -r LINE; do
    case "$LINE" in
      NAME=*)    NAME="${LINE#NAME=}" ;;
      ROUTE=*)   ROUTE="${LINE#ROUTE=}" ;;
      METHOD=*)  METHOD="${LINE#METHOD=}" ;;
      THREADS=*) THREADS="${LINE#THREADS=}" ;;
      CONNS=*)   CONNS="${LINE#CONNS=}" ;;
      DUR=*)     DUR="${LINE#DUR=}" ;;
      HDR=*)     HDR_ARGS+=("-H" "${LINE#HDR=}") ;;
    esac
  done <<< "$SCEN"

  URL="http://127.0.0.1:$PORT$ROUTE"
  RAW="$(wrk -t"$THREADS" -c"$CONNS" -d"$DUR" --latency "${HDR_ARGS[@]}" "$URL" 2>&1)" \
    || { echo "  $NAME: wrk failed" >&2; echo "$RAW" >&2; continue; }

  # Parse rps + latency percentiles from wrk output
  RPS="$(echo "$RAW" | awk '/Requests\/sec:/ {print $2}')"
  P50="$(echo "$RAW" | awk '/50%/  && NF==2 {print $2}' | head -1)"
  P75="$(echo "$RAW" | awk '/75%/  && NF==2 {print $2}' | head -1)"
  P90="$(echo "$RAW" | awk '/90%/  && NF==2 {print $2}' | head -1)"
  P99="$(echo "$RAW" | awk '/99%/  && NF==2 {print $2}' | head -1)"

  # Normalise latency units → ms. wrk emits values like "521.00us",
  # "2.44ms", "1.05s", "1.20m" — split into (number, unit) with a
  # single sed to avoid the `${var%[a-z]*}` pitfall (that pattern only
  # strips one letter, leaving "521.00u" and quietly corrupting the
  # downstream JSON).
  norm() {
    local v="$1"
    local n u
    n="$(printf '%s' "$v" | sed -E 's/^([0-9.]+).*$/\1/')"
    u="$(printf '%s' "$v" | sed -E 's/^[0-9.]+//')"
    case "$u" in
      us) awk "BEGIN{print $n/1000}" ;;
      ms) echo "$n" ;;
      s)  awk "BEGIN{print $n*1000}" ;;
      m)  awk "BEGIN{print $n*60000}" ;;
      "") echo "$n" ;;
      *)  echo "$n" ;;
    esac
  }
  P50_MS="$(norm "$P50")"
  P75_MS="$(norm "$P75")"
  P90_MS="$(norm "$P90")"
  P99_MS="$(norm "$P99")"

  printf "  %-22s  rps=%-10s  p50=%sms  p99=%sms\n" "$NAME" "${RPS:-?}" "${P50_MS:-?}" "${P99_MS:-?}" >&2

  # Merge into the results JSON
  jq --arg n "$NAME" \
     --argjson rps "${RPS:-0}" \
     --argjson p50 "${P50_MS:-0}" \
     --argjson p75 "${P75_MS:-0}" \
     --argjson p90 "${P90_MS:-0}" \
     --argjson p99 "${P99_MS:-0}" \
     '.scenarios[$n] = {rps:$rps, p50_ms:$p50, p75_ms:$p75, p90_ms:$p90, p99_ms:$p99}' \
     "$RESULTS_TMP" > "$RESULTS_TMP.new"
  mv "$RESULTS_TMP.new" "$RESULTS_TMP"
done

# Finalise output with metadata
GIT_SHA="$(git rev-parse --short HEAD)"
VERSION="$(sed -n '3p' Cargo.toml | sed -E 's/.*"(.+)".*/\1/')"
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
KERNEL="$(uname -sr)"
CPUS="$(nproc)"

jq --arg sha "$GIT_SHA" \
   --arg ver "$VERSION" \
   --arg now "$NOW" \
   --arg kern "$KERNEL" \
   --argjson cpus "$CPUS" \
   '. + {captured_at:$now, git_sha:$sha, version:$ver, kernel:$kern, cpus:$cpus}' \
   "$RESULTS_TMP" > "$OUTPUT"

rm -f "$RESULTS_TMP"
echo ">> wrote $OUTPUT"

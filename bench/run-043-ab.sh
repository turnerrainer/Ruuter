#!/usr/bin/env bash
# 043 A/B harness — brings up a second ruuter instance as the "target"
# (TCP:8082 + UDS:/tmp/bench-side.sock) then runs the main bench
# against the 043-uds and 043-tcp scenarios.
#
# The MAIN bench server (started by bench/run.sh at port 8081) needs
# unix_socket_map: side → /tmp/bench-side.sock. We inject that into
# the tempconfig by pointing run.sh at a config we control.

set -eu

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

SIDE_SOCK="/tmp/ruuter-bench-side.sock"
SIDE_CFG="$(mktemp -t ruuter-side.XXXXXX.yaml)"
MAIN_CFG="$(mktemp -t ruuter-main.XXXXXX.yaml)"
SIDE_LOG="$(mktemp -t ruuter-side.XXXXXX.log)"
MAIN_LOG="$(mktemp -t ruuter-main.XXXXXX.log)"

rm -f "$SIDE_SOCK"
cat > "$SIDE_CFG" <<EOF
port: 8080
config_path: ./DSL
listeners:
  - name: side-tcp
    bind: "127.0.0.1:8082"
  - name: side-uds
    unix: "$SIDE_SOCK"
EOF

cat > "$MAIN_CFG" <<EOF
port: 8081
config_path: ./DSL
unix_socket_map:
  side: "$SIDE_SOCK"
EOF

cleanup() {
  [ -n "${SIDE_PID:-}" ] && kill -9 "$SIDE_PID" >/dev/null 2>&1 || true
  [ -n "${MAIN_PID:-}" ] && kill -9 "$MAIN_PID" >/dev/null 2>&1 || true
  rm -f "$SIDE_CFG" "$MAIN_CFG" "$SIDE_SOCK"
}
trap cleanup EXIT

BIN="$REPO_ROOT/target/release/ruuter-on-rust"

echo ">> starting SIDE server (TCP:8082 + UDS $SIDE_SOCK)"
RUUTER_ADMIN_ENABLED=false "$BIN" --config "$SIDE_CFG" --dsl DSL --constants constants.ini > "$SIDE_LOG" 2>&1 &
SIDE_PID=$!
for _ in $(seq 1 20); do
  curl -sf http://127.0.0.1:8082/health >/dev/null 2>&1 && break
  sleep 0.5
done
if ! curl -sf http://127.0.0.1:8082/health >/dev/null 2>&1; then
  echo "SIDE failed to start; log:" >&2; tail "$SIDE_LOG" >&2; exit 1
fi
[ -S "$SIDE_SOCK" ] || { echo "SIDE UDS not bound at $SIDE_SOCK" >&2; exit 1; }

echo ">> starting MAIN server (TCP:8081, alias side → $SIDE_SOCK)"
RUUTER_ADMIN_ENABLED=false "$BIN" --config "$MAIN_CFG" --dsl DSL --constants constants.ini > "$MAIN_LOG" 2>&1 &
MAIN_PID=$!
for _ in $(seq 1 20); do
  curl -sf http://127.0.0.1:8081/health >/dev/null 2>&1 && break
  sleep 0.5
done
if ! curl -sf http://127.0.0.1:8081/health >/dev/null 2>&1; then
  echo "MAIN failed to start; log:" >&2; tail "$MAIN_LOG" >&2; exit 1
fi

# Warm both callers
curl -sf http://127.0.0.1:8081/samples/bench-uds-caller >/dev/null 2>&1 || true
curl -sf http://127.0.0.1:8081/samples/bench-tcp-caller >/dev/null 2>&1 || true

echo "=== 043 A/B ==="
for ROUTE in /samples/bench-uds-caller /samples/bench-tcp-caller; do
  RESULT=$(wrk -t4 -c64 -d10s --latency "http://127.0.0.1:8081$ROUTE" 2>&1)
  RPS=$(echo "$RESULT" | awk '/Requests\/sec:/ {print $2}')
  P50=$(echo "$RESULT" | awk '/50%/  && NF==2 {print $2}' | head -1)
  P99=$(echo "$RESULT" | awk '/99%/  && NF==2 {print $2}' | head -1)
  printf "  %-40s rps=%-10s p50=%s p99=%s\n" "$ROUTE" "${RPS:-?}" "${P50:-?}" "${P99:-?}"
done

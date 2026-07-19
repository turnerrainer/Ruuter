#!/usr/bin/env bash
# 049 A/B harness — h2c UDS vs h1.1 UDS.
#
# Spins up TWO second-instance ruuter servers on separate sockets:
#   - side-h1.sock: HTTP/1.1 (task 043 baseline)
#   - side-h2.sock: h2c (task 049)
# And configures the main server with unix_socket_map aliasing both.
#
# Two caller DSLs (both already exist as bench-uds-caller variants
# via alias) hit each. We can't easily switch the main's uds_http_
# version per-request from wrk, so this harness runs the entire
# main server TWICE:
#   - Once with uds_http_version=http1 → hits h1 target
#   - Once with uds_http_version=http2 → hits h2 target
# Same DSL, different config, different transport.

set -eu
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

SOCK_H1="/tmp/ruuter-bench-side-h1.sock"
SOCK_H2="/tmp/ruuter-bench-side-h2.sock"
SIDE_H1_CFG="$(mktemp -t ruuter-side-h1.XXXXXX.yaml)"
SIDE_H2_CFG="$(mktemp -t ruuter-side-h2.XXXXXX.yaml)"
MAIN_H1_CFG="$(mktemp -t ruuter-main-h1.XXXXXX.yaml)"
MAIN_H2_CFG="$(mktemp -t ruuter-main-h2.XXXXXX.yaml)"

rm -f "$SOCK_H1" "$SOCK_H2"

cat > "$SIDE_H1_CFG" <<EOF
port: 8083
config_path: ./DSL
listeners:
  - name: h1-uds
    unix: "$SOCK_H1"
    http2: false
EOF
cat > "$SIDE_H2_CFG" <<EOF
port: 8084
config_path: ./DSL
listeners:
  - name: h2-uds
    unix: "$SOCK_H2"
    http2: true
EOF

cat > "$MAIN_H1_CFG" <<EOF
port: 8081
config_path: ./DSL
uds_http_version: http1
unix_socket_map:
  side: "$SOCK_H1"
EOF
cat > "$MAIN_H2_CFG" <<EOF
port: 8081
config_path: ./DSL
uds_http_version: http2
unix_socket_map:
  side: "$SOCK_H2"
EOF

SIDE_H1_LOG="$(mktemp)"
SIDE_H2_LOG="$(mktemp)"

cleanup() {
  for pid in ${SIDE_H1_PID:-} ${SIDE_H2_PID:-} ${MAIN_PID:-}; do
    kill -9 "$pid" >/dev/null 2>&1 || true
  done
  rm -f "$SIDE_H1_CFG" "$SIDE_H2_CFG" "$MAIN_H1_CFG" "$MAIN_H2_CFG" "$SOCK_H1" "$SOCK_H2"
}
trap cleanup EXIT

BIN="$REPO_ROOT/target/release/ruuter-on-rust"

echo ">> starting h1 UDS side server ($SOCK_H1)"
RUUTER_ADMIN_ENABLED=false "$BIN" --config "$SIDE_H1_CFG" --dsl DSL --constants constants.ini > "$SIDE_H1_LOG" 2>&1 &
SIDE_H1_PID=$!

echo ">> starting h2c UDS side server ($SOCK_H2)"
RUUTER_ADMIN_ENABLED=false "$BIN" --config "$SIDE_H2_CFG" --dsl DSL --constants constants.ini > "$SIDE_H2_LOG" 2>&1 &
SIDE_H2_PID=$!

# Wait until both sockets exist
for _ in $(seq 1 20); do
  if [ -S "$SOCK_H1" ] && [ -S "$SOCK_H2" ]; then break; fi
  sleep 0.5
done
if ! [ -S "$SOCK_H1" ]; then echo "h1 side never bound"; tail "$SIDE_H1_LOG"; exit 1; fi
if ! [ -S "$SOCK_H2" ]; then echo "h2 side never bound"; tail "$SIDE_H2_LOG"; exit 1; fi

run_main_and_bench() {
  local cfg="$1"; local label="$2"
  MAIN_LOG="$(mktemp)"
  RUUTER_ADMIN_ENABLED=false "$BIN" --config "$cfg" --dsl DSL --constants constants.ini > "$MAIN_LOG" 2>&1 &
  MAIN_PID=$!
  for _ in $(seq 1 20); do
    curl -sf http://127.0.0.1:8081/health >/dev/null 2>&1 && break
    sleep 0.5
  done
  if ! curl -sf http://127.0.0.1:8081/health >/dev/null 2>&1; then
    echo "MAIN ($label) never started; log:"; tail "$MAIN_LOG"; exit 1
  fi
  # Warm-up
  curl -sf http://127.0.0.1:8081/samples/bench-uds-caller >/dev/null 2>&1 || true

  RESULT=$(wrk -t4 -c64 -d10s --latency "http://127.0.0.1:8081/samples/bench-uds-caller" 2>&1)
  RPS=$(echo "$RESULT" | awk '/Requests\/sec:/ {print $2}')
  P50=$(echo "$RESULT" | awk '/50%/  && NF==2 {print $2}' | head -1)
  P99=$(echo "$RESULT" | awk '/99%/  && NF==2 {print $2}' | head -1)
  printf "  %-20s rps=%-10s p50=%s p99=%s\n" "$label" "${RPS:-?}" "${P50:-?}" "${P99:-?}"

  kill -9 $MAIN_PID >/dev/null 2>&1 || true
  MAIN_PID=""
  sleep 1
}

echo "=== 049 A/B ==="
run_main_and_bench "$MAIN_H1_CFG" "uds-h1-pool"
run_main_and_bench "$MAIN_H2_CFG" "uds-h2c-pool"

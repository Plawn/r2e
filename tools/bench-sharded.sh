#!/usr/bin/env bash
#
# bench-sharded.sh — benchmark R2E SO_REUSEPORT sharded serving (`server.workers`)
# against the default multi-thread runtime.
#
# It builds the `example-sharded-bench` app in --release, then for each mode
# (default multi-thread; workers=per-core) starts the server, waits for the
# port, runs `oha` against three endpoints (/plain, /json, /db), collects
# RPS and p50/p99 latency, kills the server, and prints a markdown table.
#
# The serve mode is switched WITHOUT rebuilding via the R2E_SERVER_WORKERS env
# var (config overlay → server.workers):
#   - unset            → default single multi-thread runtime
#   - per-core         → SO_REUSEPORT sharding, one current_thread worker per core
#
# Requirements: oha, jq (or python3), curl, a release build toolchain.
#
# Usage:
#   tools/bench-sharded.sh                # default params (10s @ 64 conns)
#   DURATION=20s CONNS=128 tools/bench-sharded.sh
#
set -euo pipefail

# ---- Parameters (same across both modes) -----------------------------------
HOST="127.0.0.1"
PORT="${PORT:-3000}"
DURATION="${DURATION:-10s}"
WARMUP="${WARMUP:-2s}"
CONNS="${CONNS:-64}"
ENDPOINTS=(/plain /json /db)

# oha is resolved from PATH so the script runs unchanged on Linux (override
# with OHA_BIN=/path/to/oha if needed).
OHA_BIN="${OHA_BIN:-$(command -v oha || true)}"
if [[ -z "$OHA_BIN" ]]; then
  echo "ERROR: oha not found in PATH (install it, or set OHA_BIN=/path/to/oha)" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/examples/example-sharded-bench"
# Resolve the workspace target directory (may be overridden globally, e.g.
# ~/.cargo/target) rather than assuming $REPO_ROOT/target.
TARGET_DIR="$(cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 2>/dev/null \
  | (jq -r '.target_directory' 2>/dev/null || python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])'))"
TARGET_DIR="${TARGET_DIR:-$REPO_ROOT/target}"
BIN="$TARGET_DIR/release/example-sharded-bench"

SERVER_PID=""
RESULTS_DIR=""

stop_server() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  # Belt and suspenders: kill any stray bench server.
  pkill -f "release/example-sharded-bench" 2>/dev/null || true
}

on_exit() {
  # Runs on ANY exit (including early failures): kill the server and remove
  # scratch files. /tmp/bench-server.log is kept for post-mortem.
  stop_server
  [[ -n "$RESULTS_DIR" ]] && rm -rf "$RESULTS_DIR"
  rm -f /tmp/bench-*.json
  # The bench app creates its sqlite DB in the system temp dir.
  rm -f "${TMPDIR:-/tmp}/r2e-sharded-bench.db"
}
trap on_exit EXIT INT TERM

# ---- Helpers ----------------------------------------------------------------

# Extract requestsPerSec, p50, p99 (seconds) from an oha --output-format json file.
# Emits: "<rps> <p50_seconds> <p99_seconds>"
parse_oha() {
  local file="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r '"\(.summary.requestsPerSec) \(.latencyPercentiles.p50) \(.latencyPercentiles.p99)"' "$file"
  else
    python3 - "$file" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(d["summary"]["requestsPerSec"], d["latencyPercentiles"]["p50"], d["latencyPercentiles"]["p99"])
PY
  fi
}

free_port() {
  # Kill anything currently bound to our port (a stray bench server from a
  # previous run, hung curl, etc.) so the server can bind cleanly.
  if command -v lsof >/dev/null 2>&1; then
    lsof -ti tcp:"$PORT" 2>/dev/null | xargs -r kill -9 2>/dev/null || true
  fi
}

wait_for_port() {
  local tries=0
  until curl -fsS "http://$HOST:$PORT/plain" >/dev/null 2>&1; do
    # Detect an early server death (e.g. AddrInUse panic) instead of polling
    # a dead process for the full timeout.
    if [[ -n "$SERVER_PID" ]] && ! kill -0 "$SERVER_PID" 2>/dev/null; then
      echo "ERROR: server process exited before becoming ready; log:" >&2
      tail -5 /tmp/bench-server.log >&2 2>/dev/null || true
      exit 1
    fi
    tries=$((tries + 1))
    if [[ $tries -gt 100 ]]; then
      echo "ERROR: server did not become ready on $HOST:$PORT" >&2
      exit 1
    fi
    sleep 0.1
  done
}

start_server() {
  # $1: empty for default mode, or a value for R2E_SERVER_WORKERS (e.g. per-core)
  local workers="$1"
  stop_server
  free_port
  SERVER_PID=""
  # exec so $SERVER_PID is the server binary itself, not a wrapping subshell —
  # kill/kill -0 in stop_server and wait_for_port then act on the real process.
  if [[ -n "$workers" ]]; then
    ( cd "$APP_DIR" && R2E_SERVER_WORKERS="$workers" R2E_SERVER_PORT="$PORT" exec "$BIN" >/tmp/bench-server.log 2>&1 ) &
  else
    ( cd "$APP_DIR" && R2E_SERVER_PORT="$PORT" exec "$BIN" >/tmp/bench-server.log 2>&1 ) &
  fi
  SERVER_PID=$!
  wait_for_port
}

run_mode() {
  # $1: human label, $2: workers value ("" = default)
  local label="$1"
  local workers="$2"
  start_server "$workers"

  for ep in "${ENDPOINTS[@]}"; do
    # Warmup (discarded).
    "$OHA_BIN" --no-tui --output-format json -z "$WARMUP" -c "$CONNS" \
      "http://$HOST:$PORT$ep" >/dev/null 2>&1 || true
    # Measured run. A failure (bad -z value, transport error) skips this
    # endpoint — fmt_row prints "n/a" for the missing result file — instead of
    # aborting the whole run via set -e.
    local out="/tmp/bench-${label// /_}-${ep//\//_}.json"
    if ! "$OHA_BIN" --no-tui --output-format json -z "$DURATION" -c "$CONNS" \
      "http://$HOST:$PORT$ep" >"$out" 2>/dev/null; then
      echo "WARN: oha failed on $ep (mode: $label) — skipping" >&2
      continue
    fi
    read -r rps p50 p99 < <(parse_oha "$out")
    # Store results in a temp file keyed by label/endpoint (bash 3.2 on macOS
    # has no associative arrays).
    echo "$rps $p50 $p99" > "$RESULTS_DIR/${label}_${ep//\//_}"
  done

  stop_server
  SERVER_PID=""
}

fmt_row() {
  # $1 label, $2 endpoint → markdown cells "| rps | p50ms | p99ms |"
  local f="$RESULTS_DIR/${1}_${2//\//_}"
  if [[ ! -s "$f" ]]; then
    printf '| n/a | n/a | n/a '
    return
  fi
  local rps p50 p99
  read -r rps p50 p99 < "$f"
  # rps → integer; p50/p99 seconds → milliseconds (3 decimals).
  # awk handles float formatting portably (bash 3.2 printf does not).
  # LC_ALL=C so awk emits '.' decimal separators regardless of system locale.
  local rps_i p50_ms p99_ms
  rps_i=$(LC_ALL=C awk "BEGIN{printf \"%.0f\", $rps}")
  p50_ms=$(LC_ALL=C awk "BEGIN{printf \"%.3f\", $p50*1000}")
  p99_ms=$(LC_ALL=C awk "BEGIN{printf \"%.3f\", $p99*1000}")
  printf '| %s | %s | %s ' "$rps_i" "$p50_ms" "$p99_ms"
}

# ---- Machine / tool info ----------------------------------------------------

OS_NAME="$(uname -srm)"
if command -v sysctl >/dev/null 2>&1 && sysctl -n hw.ncpu >/dev/null 2>&1; then
  NCPU="$(sysctl -n hw.ncpu)"
elif command -v nproc >/dev/null 2>&1; then
  NCPU="$(nproc)"
else
  NCPU="?"
fi
OHA_VER="$("$OHA_BIN" --version 2>/dev/null || echo 'oha ?')"
RUSTC_VER="$(rustc --version 2>/dev/null || echo 'rustc ?')"

echo "== R2E sharded-serving benchmark =="
echo "OS:        $OS_NAME"
echo "CPU count: $NCPU"
echo "oha:       $OHA_VER"
echo "rustc:     $RUSTC_VER"
echo "params:    -z $DURATION -c $CONNS (warmup -z $WARMUP), endpoints: ${ENDPOINTS[*]}"
echo

# ---- Build ------------------------------------------------------------------

echo ">> building example-sharded-bench (--release) ..."
( cd "$REPO_ROOT" && cargo build --release -p example-sharded-bench )
echo

# ---- Run --------------------------------------------------------------------

RESULTS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/r2e-bench.XXXXXX")"

echo ">> mode: default (multi-thread) ..."
run_mode "default" ""

echo ">> mode: workers=per-core (SO_REUSEPORT sharding) ..."
run_mode "per-core" "per-core"

# ---- Report -----------------------------------------------------------------

echo
echo "### Results (RPS / p50 ms / p99 ms)"
echo
echo "| Endpoint | default RPS | default p50 | default p99 | per-core RPS | per-core p50 | per-core p99 |"
echo "|---|---|---|---|---|---|---|"
for ep in "${ENDPOINTS[@]}"; do
  printf '| %s ' "$ep"
  fmt_row "default" "$ep"
  fmt_row "per-core" "$ep"
  printf '|\n'
done
echo
echo "Notes: latencies in milliseconds; RPS rounded. oha params identical across modes."

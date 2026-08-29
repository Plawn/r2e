#!/usr/bin/env bash
#
# bench-hotpath.sh — stable HTTP load over the framework's hot-path wrappers,
# for the before/after table in docs/claude/hot-path-clone-audit.md (task #982).
#
# It builds `example-app` in --release (the only workspace app that stacks the
# Prometheus layer, the OpenTelemetry trace layer, JWT identity extraction, the
# `Logged`/`Timed` interceptors and the OpenAPI document in one router), starts
# it, scrapes the demo JWT it prints at boot, and runs `oha` against three
# endpoints that each exercise a different wrapper:
#
#   /mixed/public   layer stack only  (prometheus + otel + request-id + cors)
#   /users/         + JWT validation + Logged/Timed interceptors + controller
#   /openapi.json   the pre-encoded immutable document
#
# It benchmarks WHATEVER IS IN THE WORKING TREE and prints one markdown row per
# endpoint, tagged with $LABEL. To produce the "before" side, revert just the
# hot-path sources in place first — no worktree, no second checkout:
#
#   git checkout <pre-fix-rev> -- \
#       r2e-observability/src/middleware.rs r2e-openapi/src/handlers.rs \
#       r2e-prometheus/src/layer.rs r2e-prometheus/src/metrics.rs \
#       r2e-security/src/jwt.rs r2e-security/src/jwks.rs \
#       r2e-utils/src/interceptors.rs
#   LABEL=before tools/bench-hotpath.sh
#   git restore --source=HEAD --staged --worktree -- <those files>
#   LABEL=after  tools/bench-hotpath.sh
#
# Only those files differ between the two runs: the app, its config, the load
# generator and its parameters are identical, so the delta is the framework.
#
# Requirements: oha, jq (or python3), curl, a release build toolchain.
#
# Usage:
#   LABEL=after tools/bench-hotpath.sh
#   LABEL=after DURATION=20s CONNS=128 tools/bench-hotpath.sh
#
set -euo pipefail
# `printf %f` must accept the dot-decimal numbers `bc`/`jq` emit, whatever the
# operator's locale is.
export LC_ALL=C

HOST="127.0.0.1"
PORT="${PORT:-3001}"          # example-app's application.yaml server.port
DURATION="${DURATION:-10s}"
WARMUP="${WARMUP:-3s}"
CONNS="${CONNS:-64}"
LABEL="${LABEL:-working-tree}"

OHA_BIN="${OHA_BIN:-$(command -v oha || true)}"
if [[ -z "$OHA_BIN" ]]; then
  echo "ERROR: oha not found in PATH (install it, or set OHA_BIN=/path/to/oha)" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/examples/example-app"
TARGET_DIR="$(cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 2>/dev/null \
  | (jq -r '.target_directory' 2>/dev/null || python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])'))"
TARGET_DIR="${TARGET_DIR:-$REPO_ROOT/target}"
BIN="$TARGET_DIR/release/example-app"

SERVER_PID=""
RESULTS_DIR=""
SERVER_LOG="${TMPDIR:-/tmp}/bench-hotpath-server.log"

stop_server() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  pkill -f "release/example-app" 2>/dev/null || true
}

on_exit() {
  stop_server
  [[ -n "$RESULTS_DIR" ]] && rm -rf "$RESULTS_DIR"
}
trap on_exit EXIT INT TERM

# Extract requestsPerSec, p50, p99 (seconds) from an `oha --output-format json`
# report. Emits: "<rps> <p50_seconds> <p99_seconds>".
parse_oha() {
  local file="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r '"\(.summary.requestsPerSec) \(.latencyPercentiles.p50) \(.latencyPercentiles.p99)"' "$file"
  else
    python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print(d["summary"]["requestsPerSec"], d["latencyPercentiles"]["p50"], d["latencyPercentiles"]["p99"])
' "$file"
  fi
}

echo "==> Building example-app (release)"
(cd "$REPO_ROOT" && cargo build --release -p example-app)

echo "==> Starting the server"
: >"$SERVER_LOG"
(cd "$APP_DIR" && RUST_LOG=warn "$BIN" >"$SERVER_LOG" 2>&1) &
SERVER_PID=$!

for _ in $(seq 1 100); do
  if curl -fsS "http://$HOST:$PORT/mixed/public" >/dev/null 2>&1; then break; fi
  sleep 0.2
done
if ! curl -fsS "http://$HOST:$PORT/mixed/public" >/dev/null 2>&1; then
  echo "ERROR: server did not come up on $HOST:$PORT; see $SERVER_LOG" >&2
  exit 1
fi

# The demo JWT is printed on the line after the "=== Test JWT" banner.
TOKEN="$(grep -A1 'Test JWT' "$SERVER_LOG" | tail -n1 | tr -d '[:space:]')"
if [[ -z "$TOKEN" ]]; then
  echo "ERROR: could not scrape the demo JWT from $SERVER_LOG" >&2
  exit 1
fi

RESULTS_DIR="$(mktemp -d)"

run_endpoint() {
  local name="$1" path="$2"
  shift 2
  # Warm-up pass, discarded: first-touch page faults, lazily-registered metric
  # series and the JWKS/validation caches must not land in the measurement.
  "$OHA_BIN" -z "$WARMUP" -c "$CONNS" --no-tui "$@" "http://$HOST:$PORT$path" >/dev/null 2>&1 || true
  "$OHA_BIN" -z "$DURATION" -c "$CONNS" --no-tui --output-format json "$@" \
    "http://$HOST:$PORT$path" >"$RESULTS_DIR/$name.json"
}

echo "==> Load: $DURATION @ $CONNS connections (warm-up $WARMUP)"
run_endpoint public   /mixed/public
run_endpoint users    /users/ -H "Authorization: Bearer $TOKEN"
run_endpoint openapi  /openapi.json

echo
echo "| label | endpoint | wrappers exercised | req/s | p50 | p99 |"
echo "|---|---|---|---|---|---|"
emit() {
  local name="$1" endpoint="$2" what="$3"
  read -r rps p50 p99 < <(parse_oha "$RESULTS_DIR/$name.json")
  printf '| %s | `%s` | %s | %.0f | %.2f ms | %.2f ms |\n' \
    "$LABEL" "$endpoint" "$what" "$rps" \
    "$(echo "$p50 * 1000" | bc -l)" "$(echo "$p99 * 1000" | bc -l)"
}
emit public  /mixed/public "prometheus + otel layers"
emit users   /users/       "+ JWT validation + Logged/Timed"
emit openapi /openapi.json "immutable document"

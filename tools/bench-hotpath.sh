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
#   /mixed/public   layer stack + a controller handler returning the user list
#   /users/         + JWT validation + Logged/Timed interceptors + controller
#   /openapi.json   the pre-encoded immutable document
#
# The server it measures is provably its own: the port must be free before the
# launch, the launched PID must still be alive after readiness, and the app is
# started with a unique per-run marker (`R2E_APP_GREETING`) that the script
# reads back from `GET /config`. Every measured endpoint is also probed for a
# 2xx before the run and its `oha` status distribution is verified afterwards,
# so a 401/500 storm can never be reported as throughput.
#
# It benchmarks WHATEVER IS IN THE WORKING TREE, unless BEFORE_REV is set, in
# which case it checks the hot-path sources out of that revision first and
# restores them on exit (including on Ctrl-C) — see "Before/after" below.
#
# Requirements: oha, curl, awk, jq (or python3), a release build toolchain.
#
# Usage:
#   LABEL=after tools/bench-hotpath.sh
#   LABEL=after DURATION=20s CONNS=128 tools/bench-hotpath.sh
#
# Before/after (needs a CLEAN work tree — the script refuses to run otherwise):
#   LABEL=before BEFORE_REV=d046b84 tools/bench-hotpath.sh   # reverts + restores
#   LABEL=after                     tools/bench-hotpath.sh
#
set -euo pipefail
# `printf %f` must accept the dot-decimal numbers `jq`/`awk` emit, whatever the
# operator's locale is.
export LC_ALL=C

HOST="127.0.0.1"
PORT="${PORT:-3001}"          # example-app's application.yaml server.port
DURATION="${DURATION:-10s}"
WARMUP="${WARMUP:-3s}"
CONNS="${CONNS:-64}"
LABEL="${LABEL:-working-tree}"
BEFORE_REV="${BEFORE_REV:-}"

# The only files that may differ between a "before" and an "after" run. The app,
# its config, the load generator and its parameters are identical, so the delta
# is the framework and nothing else.
HOTPATH_SOURCES=(
  r2e-observability/src/middleware.rs
  r2e-openapi/src/handlers.rs
  r2e-prometheus/src/layer.rs
  r2e-prometheus/src/metrics.rs
  r2e-security/src/jwt.rs
  r2e-security/src/jwks.rs
  r2e-utils/src/interceptors.rs
)

OHA_BIN="${OHA_BIN:-$(command -v oha || true)}"
if [[ -z "$OHA_BIN" ]]; then
  echo "ERROR: oha not found in PATH (install it, or set OHA_BIN=/path/to/oha)" >&2
  exit 1
fi
for tool in curl awk; do
  command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: $tool not found in PATH" >&2; exit 1; }
done
if ! command -v jq >/dev/null 2>&1 && ! command -v python3 >/dev/null 2>&1; then
  echo "ERROR: need jq or python3 to read oha's JSON report" >&2
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
SOURCES_REVERTED=0
SERVER_LOG="${TMPDIR:-/tmp}/bench-hotpath-server-$$.log"
# Unique per run: proves the server answering on $PORT is the one we launched.
MARKER="bench-hotpath-$$-${RANDOM}${RANDOM}"

# ---------------------------------------------------------------------------
# Lifecycle
# ---------------------------------------------------------------------------

stop_server() {
  # Only ever our own child. `pkill -f release/example-app` would take out a
  # concurrent benchmark or an unrelated local release build.
  [[ -n "$SERVER_PID" ]] || return 0
  kill -0 "$SERVER_PID" 2>/dev/null || return 0
  kill -TERM "$SERVER_PID" 2>/dev/null || true
  for _ in $(seq 1 50); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
  done
  kill -KILL "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
}

restore_sources() {
  [[ "$SOURCES_REVERTED" == 1 ]] || return 0
  SOURCES_REVERTED=0
  echo "==> Restoring the hot-path sources to HEAD"
  (cd "$REPO_ROOT" && git restore --source=HEAD --staged --worktree -- "${HOTPATH_SOURCES[@]}")
}

on_exit() {
  stop_server
  restore_sources
  [[ -n "$RESULTS_DIR" ]] && rm -rf "$RESULTS_DIR"
  return 0
}
trap on_exit EXIT
# Re-raise so the exit status reflects the signal; EXIT does the cleanup.
trap 'exit 130' INT
trap 'exit 143' TERM

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# True when something is already listening on $HOST:$PORT.
port_in_use() {
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1
  else
    (exec 3<>"/dev/tcp/$HOST/$PORT") 2>/dev/null
  fi
}

# The PIDs listening on $PORT, when lsof is available (empty otherwise).
listening_pids() {
  command -v lsof >/dev/null 2>&1 || return 0
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null || true
}

json_get() {
  local file="$1" filter="$2" py="$3"
  if command -v jq >/dev/null 2>&1; then
    jq -r "$filter" "$file"
  else
    python3 -c "$py" "$file"
  fi
}

# Extract requestsPerSec, p50, p99 (seconds) from an `oha --output-format json`
# report. Emits: "<rps> <p50_seconds> <p99_seconds>".
parse_oha() {
  json_get "$1" \
    '"\(.summary.requestsPerSec) \(.latencyPercentiles.p50) \(.latencyPercentiles.p99)"' \
    'import json,sys
d=json.load(open(sys.argv[1]))
print(d["summary"]["requestsPerSec"], d["latencyPercentiles"]["p50"], d["latencyPercentiles"]["p99"])'
}

# Fail unless every response in the run was 2xx and no transport error occurred.
# A benchmark of a 401 or 500 loop measures the error path, not the hot path.
assert_all_2xx() {
  local file="$1" name="$2" report rate bad=0 entry
  report="$(json_get "$file" \
    '[((.statusCodeDistribution // {}) | to_entries[] | "\(.key)=\(.value)"),
      ((.errorDistribution // {}) | to_entries[] | "error(\(.key))=\(.value)")] | join(" ")' \
    'import json,sys
d=json.load(open(sys.argv[1]))
parts=["%s=%s"%(k,v) for k,v in (d.get("statusCodeDistribution") or {}).items()]
parts+=["error(%s)=%s"%(k,v) for k,v in (d.get("errorDistribution") or {}).items()]
print(" ".join(parts))')"

  if [[ -z "$report" ]]; then
    # An oha build that names the field differently: fall back to the summary's
    # success rate rather than passing a run we could not inspect.
    rate="$(json_get "$file" '.summary.successRate // 0' \
      'import json,sys
d=json.load(open(sys.argv[1]))
print((d.get("summary") or {}).get("successRate", 0))')"
    if awk -v r="$rate" 'BEGIN { exit !(r >= 1) }'; then
      echo "    $name: successRate=$rate (no status distribution in this oha build)"
      return 0
    fi
    echo "ERROR: $name has no status distribution and successRate=$rate" >&2
    exit 1
  fi

  for entry in $report; do
    case "$entry" in
      2[0-9][0-9]=*) ;;
      *) bad=1 ;;
    esac
  done
  if [[ "$bad" == 1 ]]; then
    echo "ERROR: $name did not answer 2xx for every request: $report" >&2
    exit 1
  fi
  echo "    $name: $report"
}

die_with_log() {
  echo "ERROR: $1" >&2
  echo "--- last 20 lines of $SERVER_LOG ---" >&2
  tail -n 20 "$SERVER_LOG" >&2 || true
  exit 1
}

# ---------------------------------------------------------------------------
# Optional "before" reconstruction
# ---------------------------------------------------------------------------

if [[ -n "$BEFORE_REV" ]]; then
  if ! (cd "$REPO_ROOT" && git diff --quiet && git diff --cached --quiet); then
    echo "ERROR: BEFORE_REV needs a clean work tree — the reconstruction overwrites" >&2
    echo "       ${HOTPATH_SOURCES[*]}" >&2
    echo "       and restores them from HEAD afterwards, which would discard local edits." >&2
    exit 1
  fi
  echo "==> Reverting the hot-path sources to $BEFORE_REV (restored on exit)"
  (cd "$REPO_ROOT" && git checkout "$BEFORE_REV" -- "${HOTPATH_SOURCES[@]}")
  SOURCES_REVERTED=1
fi

# ---------------------------------------------------------------------------
# Build, launch, verify identity
# ---------------------------------------------------------------------------

RESULTS_DIR="$(mktemp -d)"

echo "==> Building example-app (release)"
(cd "$REPO_ROOT" && cargo build --release -p example-app)

if port_in_use; then
  echo "ERROR: $HOST:$PORT is already in use (pids: $(listening_pids | tr '\n' ' '))." >&2
  echo "       Refusing to benchmark a server this script did not start." >&2
  echo "       Stop it, or re-run with PORT=<free port>." >&2
  exit 1
fi

echo "==> Starting the server on $PORT"
: >"$SERVER_LOG"
# `exec` so $SERVER_PID is the app itself, not an intermediate shell: the PID
# checks and the TERM below must reach the process that owns the port.
(cd "$APP_DIR" && exec env RUST_LOG=warn R2E_SERVER_PORT="$PORT" R2E_APP_GREETING="$MARKER" \
  "$BIN" >"$SERVER_LOG" 2>&1) &
SERVER_PID=$!

ready=0
for _ in $(seq 1 100); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    die_with_log "the server exited before it became ready (pid $SERVER_PID)"
  fi
  if curl -fsS "http://$HOST:$PORT/mixed/public" >/dev/null 2>&1; then ready=1; break; fi
  sleep 0.2
done
[[ "$ready" == 1 ]] || die_with_log "server did not come up on $HOST:$PORT"

kill -0 "$SERVER_PID" 2>/dev/null || die_with_log "the server died right after becoming ready"

# Identity check: the app echoes `app.greeting` on /config, and we booted it
# with a marker no other process can be carrying.
CONFIG_BODY="$RESULTS_DIR/config.json"
curl -fsS -o "$CONFIG_BODY" "http://$HOST:$PORT/config" 2>/dev/null || : >"$CONFIG_BODY"
SERVED_MARKER="$(json_get "$CONFIG_BODY" '.greeting // ""' \
  'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("greeting", ""))
except Exception:
    print("")' 2>/dev/null || true)"
if [[ "$SERVED_MARKER" != "$MARKER" ]]; then
  die_with_log "the server on $HOST:$PORT is not the one we launched \
(GET /config greeting = '${SERVED_MARKER:-<none>}', expected '$MARKER')"
fi

# Belt and braces where lsof exists: the listening socket must belong to us.
OWNERS="$(listening_pids)"
if [[ -n "$OWNERS" ]] && ! grep -qx "$SERVER_PID" <<<"$OWNERS"; then
  die_with_log "port $PORT is held by pid(s) $(tr '\n' ' ' <<<"$OWNERS"), not by our server ($SERVER_PID)"
fi

# The demo JWT is printed on the line after the "=== Test JWT" banner.
TOKEN="$(grep -A1 'Test JWT' "$SERVER_LOG" | tail -n1 | tr -d '[:space:]')"
if [[ -z "$TOKEN" ]]; then
  die_with_log "could not scrape the demo JWT from $SERVER_LOG"
fi

# Pre-flight: every endpoint we are about to hammer must answer 2xx *now*,
# with the exact headers the load run will use.
preflight() {
  local path="$1"
  shift
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' "$@" "http://$HOST:$PORT$path")"
  [[ "$code" =~ ^2[0-9][0-9]$ ]] \
    || die_with_log "pre-flight GET $path answered $code (expected 2xx)"
}
preflight /mixed/public
preflight /users/ -H "Authorization: Bearer $TOKEN"
preflight /openapi.json

run_endpoint() {
  local name="$1" path="$2"
  shift 2
  # Warm-up pass, discarded: first-touch page faults, lazily-registered metric
  # series and the JWKS/validation caches must not land in the measurement.
  "$OHA_BIN" -z "$WARMUP" -c "$CONNS" --no-tui "$@" "http://$HOST:$PORT$path" >/dev/null 2>&1 || true
  "$OHA_BIN" -z "$DURATION" -c "$CONNS" --no-tui --output-format json "$@" \
    "http://$HOST:$PORT$path" >"$RESULTS_DIR/$name.json"
  kill -0 "$SERVER_PID" 2>/dev/null || die_with_log "the server died during the $name run"
  assert_all_2xx "$RESULTS_DIR/$name.json" "$name"
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
  # awk, not bc: one fewer requirement, and it is in POSIX.
  printf '| %s | `%s` | %s | %.0f | %.2f ms | %.2f ms |\n' \
    "$LABEL" "$endpoint" "$what" "$rps" \
    "$(awk -v v="$p50" 'BEGIN { printf "%.6f", v * 1000 }')" \
    "$(awk -v v="$p99" 'BEGIN { printf "%.6f", v * 1000 }')"
}
emit public  /mixed/public "prometheus + otel layers + handler"
emit users   /users/       "+ JWT validation + Logged/Timed"
emit openapi /openapi.json "immutable document"

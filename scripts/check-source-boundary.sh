#!/usr/bin/env bash
#
# check-source-boundary.sh — freeze the *source* side of the runtime/HTTP
# dependency boundary (Phase 0 of plans/runtime-http-dependency-containment.md).
#
# Counts, per git-tracked file under a crate's src/ directory, how many times
# the source NAMES the runtime / HTTP layer directly:
#
#   tokio      : tokio::, tokio_util::, tokio_stream::   (#[tokio::main] included)
#   axum       : axum::
#   axum-serve : CALLS of the backend's serve API (into_make_service*(, tap_io(,
#                with_graceful_shutdown() plus the ListenerExt name —
#                r2e_http::server::serve is the only caller
#                (plans/axum-containment-roadmap.md, phase A).
#   tower_http : tower_http:: — r2e-http/src/layers.rs is the destination
#                (plans/axum-containment-roadmap.md, phase B); r2e-grpc's
#                grpc-web arm is the one by-design exception (it fronts tonic,
#                not the HTTP router). `tower::` is neutral and NOT counted (D1).
#   serde_json : serde_json::{to_vec,to_string,to_string_pretty,to_writer,
#                from_slice,from_str,from_reader} — the CODEC calls only.
#                `serde_json::Value` / `json!` / `to_value` / `from_value` are
#                the workspace's dynamic-tree type and are deliberately NOT
#                counted (plans/json-codec-containment.md §1.3). Typed
#                (de)serialization goes through `r2e_core::json`.
#
# Excluded: vendor/, examples/, docs/, any tests/ directory, .claude/ — those are
# NOT part of the boundary (test harnesses and user-facing examples may name
# the runtime freely).
#
# One group looks at the OTHER side of that line — git-tracked .rs files under
# any tests/ directory (examples included):
#
#   tokio-test : `#[tokio::test]`. Async tests use `#[r2e_core::test]`
#                (`#[r2e::test]` where the crate depends on the facade): same
#                multi-thread runtime, tracing initialised, `app = …` boot and
#                bean injection (docs/claude/testing-conventions.md, "Async test
#                attribute"). Crates BELOW r2e-core in the graph cannot reach the
#                macro and are allowlisted: r2e-rt, r2e-tracing, r2e-http,
#                r2e-macros; so is
#                r2e-devservices (dev-only harness without a core dependency).
#
# Also excluded BY DESIGN from the *tokio* group (see
# plans/runtime-http-dependency-containment.md §4 — the permanent allowlist):
#
#   r2e-rt/         it IS the runtime facade, the one crate the workspace allows
#                   to name tokio. Its occurrences are the destination of the
#                   migration, not debt to shrink; baselining them would make
#                   every line moved INTO the facade look like growth.
#   r2e-test/       the test harness owns a runtime on purpose: TestServer binds
#                   a real TcpListener and drives axum::serve, WsTestClient
#                   drives tokio-tungstenite. Routing those through the facade
#                   would buy nothing — a harness is never in a production
#                   binary — and would force runtime-shaped API into r2e-rt.
#   r2e-devservices/ same, for testcontainers: the Ryuk reaper and the
#                   workspace-session container plumbing are dev-only and speak
#                   testcontainers' own tokio-typed API.
#
#   r2e-http/src/json.rs  (serde_json group) it IS the codec facade — the
#                   one place that may call the backend's to_vec/from_slice.
#
# These exclusions are *per group*: r2e-test still counts for the axum group,
# where a harness naming axum::Router is ordinary debt like anywhere else.
#
# Each per-file count is compared against a checked-in baseline of `path:count`
# lines. The check FAILS when a count grows or when a file that was clean gains
# an occurrence. A count that went DOWN is reported as an informational note:
# shrink the baseline in the same PR.
#
# Usage:
#   scripts/check-source-boundary.sh            # check (exit 1 on growth)
#   scripts/check-source-boundary.sh --update   # rewrite the baselines from the tree
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

BOUNDARY_DIR="scripts/boundaries"
MODE="check"
if [ "${1:-}" = "--update" ]; then
    MODE="update"
elif [ -n "${1:-}" ]; then
    echo "usage: $0 [--update]" >&2
    exit 2
fi

TMPDIR_BOUNDARY=$(mktemp -d "${TMPDIR:-/tmp}/r2e-boundary.XXXXXX")
trap 'rm -rf "$TMPDIR_BOUNDARY"' EXIT

FILELIST="$TMPDIR_BOUNDARY/files.txt"
git ls-files \
    | grep -E '\.rs$' \
    | grep -E '(^|/)src/' \
    | grep -vE '^(vendor|examples|docs|\.claude)/' \
    | grep -vE '(^|/)tests/' \
    | sort >"$FILELIST"

# The tests/ side: every git-tracked .rs under a tests/ directory, examples
# included (they are the user-facing model and must show `#[r2e::test]`).
TESTLIST="$TMPDIR_BOUNDARY/test-files.txt"
git ls-files \
    | grep -E '\.rs$' \
    | grep -E '(^|/)tests/' \
    | grep -vE '^(vendor|\.claude)/' \
    | sort >"$TESTLIST"

# Per-group by-design exclusions (see the header). Empty = nothing excluded.
TOKIO_EXCLUDE='^(r2e-rt|r2e-test|r2e-devservices)/'
AXUM_EXCLUDE=''
AXUM_SHAPE_EXCLUDE='^r2e-cli/src/commands/templates/'
SERDE_JSON_EXCLUDE='^r2e-http/src/json\.rs$'
# Below r2e-core in the graph (no path to `r2e_core::test`), plus the dev-only
# testcontainers harness that deliberately has no r2e-core dependency.
TOKIO_TEST_EXCLUDE='^(r2e-rt|r2e-tracing|r2e-http|r2e-macros|r2e-devservices)/'

# ── scan: emit `path:count` for every file with >= 1 occurrence of $1 ──────
scan_group() {
    local pattern="$1" exclude="${2:-}" list="${3:-$FILELIST}"
    if [ -n "$exclude" ]; then
        local source="$list"
        list="$TMPDIR_BOUNDARY/files-filtered.txt"
        grep -vE "$exclude" "$source" >"$list" || true
    fi
    # `-H` keeps the filename prefix even when xargs hands grep a single file.
    tr '\n' '\0' <"$list" \
        | xargs -0 grep -oHE "$pattern" 2>/dev/null \
        | cut -d: -f1 \
        | sort \
        | uniq -c \
        | awk '{ print $2 ":" $1 }' \
        | sort \
        || true
}

# A fully-migrated group has an empty baseline, so `grep -v '^$'` legitimately
# matches nothing — swallow its exit 1 rather than letting pipefail kill the run.
clean_baseline() {
    sed -e 's/#.*//' -e 's/[[:space:]]*$//' "$1" | grep -v '^$' | sort || true
}

write_baseline() {
    local file="$1" group="$2" current="$3"
    {
        echo "# ${group} — source-occurrence baseline (path:count)"
        echo "#"
        if [ "$group" = "tokio-test" ]; then
            echo "# Enforced by scripts/check-source-boundary.sh over git-tracked .rs files"
            echo "# under any tests/ directory (examples included; vendor/ and .claude/"
            echo "# excluded). Crates below r2e-core (r2e-rt, r2e-tracing, r2e-http, r2e-macros) and"
            echo "# r2e-devservices are allowlisted: they cannot reach \`r2e_core::test\`."
            echo "#"
            echo "# This baseline ONLY EVER SHRINKS: async tests use \`#[r2e_core::test]\`"
            echo "# (\`#[r2e::test]\` on the facade) — docs/claude/testing-conventions.md,"
            echo "# \"Async test attribute\". A count that grows, or a new file appearing"
            echo "# here, fails CI. A count that drops is reported as a note — shrink the"
            echo "# line in the same PR so the boundary stays honest."
        else
            echo "# Enforced by scripts/check-source-boundary.sh over git-tracked files"
            echo "# under a crate src/ directory — .rs only (vendor/, examples/, docs/, tests/,"
            echo "# excluded)."
            echo "#"
            echo "# This baseline ONLY EVER SHRINKS: each migration PR of"
            echo "# plans/runtime-http-dependency-containment.md (tokio, axum) or"
            echo "# plans/json-codec-containment.md (serde_json) deletes lines from it or"
            echo "# lowers counts. A count that grows, or a new file appearing here, fails"
            echo "# CI. A count that drops is reported as a note — shrink the line in the"
            echo "# same PR so the boundary stays honest."
        fi
        echo "#"
        echo "# Regenerate with: scripts/check-source-boundary.sh --update"
        if [ "$group" = "serde_json" ]; then
            echo "#"
            echo "# Only CODEC calls are counted (to_vec/to_string/from_slice/…);"
            echo "# serde_json::Value / json! are the dynamic-tree type and are not"
            echo "# boundary debt (plan §1.3). What remains here is either a"
            echo "# Value-bound path or a by-design exception noted in the plan."
        fi
        if [ "$group" = "axum" ]; then
            echo "#"
            echo "# TWO reviewed exceptions to \"only ever shrinks\":"
            echo "# r2e-http/src/axum_compat.rs was ADDED at phase 7 (plan §5.3d,"
            echo "# decision A, 2026-08-24). That module IS the explicit escape hatch —"
            echo "# naming axum is its entire job — so it is boundary surface by design,"
            echo "# not debt to shrink."
            echo "# r2e-http/src/server.rs was ADDED by plans/axum-containment-roadmap.md"
            echo "# phase A (2026-10-24): it is the one serve path, and the occurrences it"
            echo "# carries are the ones r2e-core, sharded workers and r2e-test used to"
            echo "# reach through the \`http::serve\` re-export. Nothing else may appear"
            echo "# here without the same kind of written decision in a plan."
        fi
        if [ "$group" = "tower_http" ]; then
            echo "#"
            echo "# r2e-http/src/layers.rs is the destination, not debt. r2e-grpc's"
            echo "# grpc-web arm (feature \`web\`) is the one by-design exception: it fronts"
            echo "# tonic's routes with tower-http's CORS, not the HTTP router"
            echo "# (plans/axum-containment-roadmap.md, phase B / decision D2)."
        fi
        if [ "$group" = "axum-serve" ]; then
            echo "#"
            echo "# r2e-http/src/server.rs is the destination, not debt. The \`ListenerExt\`"
            echo "# / \`serve\` re-exports it used to share this list with were removed by"
            echo "# the routing phase (phase C) of plans/axum-containment-roadmap.md."
        fi
        echo
        cat "$current"
    } >"$file"
}

FAILED=0

check_group() {
    local group="$1" pattern="$2" baseline="$3" exclude="${4:-}" list="${5:-$FILELIST}"

    local current="$TMPDIR_BOUNDARY/current-$group.txt"
    scan_group "$pattern" "$exclude" "$list" >"$current"

    if [ "$MODE" = "update" ]; then
        write_baseline "$baseline" "$group" "$current"
        local files total
        files=$(wc -l <"$current" | tr -d ' ')
        total=$(awk -F: '{ s += $2 } END { print s + 0 }' "$current")
        echo "updated $baseline ($files files, $total occurrences)"
        return 0
    fi

    if [ ! -f "$baseline" ]; then
        echo "error: missing baseline $baseline (run $0 --update)" >&2
        FAILED=1
        return 0
    fi

    local expected="$TMPDIR_BOUNDARY/expected-$group.txt"
    clean_baseline "$baseline" >"$expected"

    local grew="$TMPDIR_BOUNDARY/grew-$group.txt"
    local shrank="$TMPDIR_BOUNDARY/shrank-$group.txt"
    : >"$grew"
    : >"$shrank"

    # files with occurrences today
    while IFS=: read -r path count; do
        [ -n "$path" ] || continue
        base=$(awk -F: -v p="$path" '$1 == p { print $2; exit }' "$expected")
        if [ -z "$base" ]; then
            echo "  + $path: $count (new — was not in the baseline)" >>"$grew"
        elif [ "$count" -gt "$base" ]; then
            echo "  ^ $path: $count (baseline $base)" >>"$grew"
        elif [ "$count" -lt "$base" ]; then
            echo "  v $path: $count (baseline $base)" >>"$shrank"
        fi
    done <"$current"

    # baseline entries that no longer match anything
    while IFS=: read -r path count; do
        [ -n "$path" ] || continue
        if ! awk -F: -v p="$path" '$1 == p { found = 1 } END { exit !found }' "$current"; then
            echo "  v $path: 0 (baseline $count — file clean or gone)" >>"$shrank"
        fi
    done <"$expected"

    if [ -s "$grew" ]; then
        FAILED=1
        echo "FAIL [$group] source occurrences grew beyond $baseline:"
        cat "$grew"
        if [ "$group" = "tokio-test" ]; then
            echo "  → Use #[r2e_core::test] (#[r2e::test] on the facade) instead of"
            echo "    #[tokio::test]: docs/claude/testing-conventions.md, \"Async test"
            echo "    attribute\". The baseline only ever shrinks."
        else
            echo "  → Go through the r2e-rt / r2e-http facade instead of naming the"
            echo "    dependency directly. The baseline only ever shrinks."
        fi
        echo
    fi

    if [ -s "$shrank" ]; then
        echo "NOTE [$group] occurrences dropped — shrink $baseline in this PR:"
        cat "$shrank"
        echo "  → Run: $0 --update"
        echo
    fi

    if [ ! -s "$grew" ] && [ ! -s "$shrank" ]; then
        local files total
        files=$(wc -l <"$current" | tr -d ' ')
        total=$(awk -F: '{ s += $2 } END { print s + 0 }' "$current")
        echo "ok   [$group] $files files / $total occurrences, matches baseline"
    fi
}

check_group "tokio" '\btokio(_util|_stream)?::' "$BOUNDARY_DIR/src-baseline-tokio.txt" "$TOKIO_EXCLUDE"
check_group "axum" '\baxum::' "$BOUNDARY_DIR/src-baseline-axum.txt" "$AXUM_EXCLUDE"
# The backend's *serve* API. r2e_http::server::serve is the one caller;
# everything else — production paths, sharded workers, TestServer — goes
# through it (plans/axum-containment-roadmap.md, phase A). Phase C removed the
# `ListenerExt` re-exports; `r2e-http/src/server.rs` is the only caller left.
check_group "axum-serve" '\b(into_make_service(_with_connect_info)?|with_graceful_shutdown|tap_io)\(|\bListenerExt\b' "$BOUNDARY_DIR/src-baseline-axum-serve.txt" ""
# tower-http is confined to r2e-http (`layers`) and r2e-grpc's grpc-web arm
# (plans/axum-containment-roadmap.md, phase B). `tower::` is neutral (D1).
check_group "tower_http" '\btower_http::' "$BOUNDARY_DIR/src-baseline-tower_http.txt" ""
# The backend's routing *shapes*: `Router`, `MethodRouter`, `middleware::from_fn`.
# Everything outside r2e-http builds on R2E's `Routes` / `MethodRoutes` /
# `middleware_fn` (plans/axum-containment-roadmap.md, phase C); the bridge is
# `r2e_http::axum_compat`. The CLI's "coming from axum" template table is prose.
check_group "axum-shape" '\bRouter\s*(<|::)|\bMethodRouter\b|\bmiddleware::from_fn(_with_state)?\b' "$BOUNDARY_DIR/src-baseline-axum-shape.txt" "$AXUM_SHAPE_EXCLUDE"
check_group "serde_json" '\bserde_json::(to_vec|to_string|to_string_pretty|to_writer|from_slice|from_str|from_reader)\b' "$BOUNDARY_DIR/src-baseline-serde_json.txt" "$SERDE_JSON_EXCLUDE"
# The tests/ side: the async test attribute is `#[r2e_core::test]`
# (docs/claude/testing-conventions.md, "Async test attribute").
check_group "tokio-test" '#\[tokio::test\b' "$BOUNDARY_DIR/tests-baseline-tokio-test.txt" "$TOKIO_TEST_EXCLUDE" "$TESTLIST"

if [ "$FAILED" -ne 0 ]; then
    echo "source boundary check FAILED — see plans/runtime-http-dependency-containment.md (tokio/axum), plans/json-codec-containment.md (serde_json) or docs/claude/testing-conventions.md (tokio-test)" >&2
    exit 1
fi

exit 0

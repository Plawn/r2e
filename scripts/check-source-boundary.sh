#!/usr/bin/env bash
#
# check-source-boundary.sh — freeze the *source* side of the runtime/HTTP
# dependency boundary (Phase 0 of plans/runtime-http-dependency-containment.md).
#
# Counts, per git-tracked file under a crate's src/ directory, how many times
# the source NAMES the runtime / HTTP layer directly:
#
#   tokio : tokio::, tokio_util::, tokio_stream::   (#[tokio::main] included)
#   axum  : axum::
#
# Excluded: vendor/, examples/, docs/, any tests/ directory, .claude/ — those are
# NOT part of the boundary (test harnesses and user-facing examples may name
# the runtime freely).
#
# Also excluded BY DESIGN: r2e-rt/ — it IS the runtime facade, the one crate the
# workspace allows to name tokio (plans/runtime-http-dependency-containment.md
# §4). Its occurrences are the destination of the migration, not debt to shrink;
# baselining them would make every line moved INTO the facade look like growth.
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

TMPDIR_BOUNDARY=$(mktemp -d)
trap 'rm -rf "$TMPDIR_BOUNDARY"' EXIT

FILELIST="$TMPDIR_BOUNDARY/files.txt"
git ls-files \
    | grep -E '\.rs$' \
    | grep -E '(^|/)src/' \
    | grep -vE '^(vendor|examples|docs|\.claude)/' \
    | grep -vE '(^|/)tests/' \
    | grep -vE '^r2e-rt/' \
    | sort >"$FILELIST"

# ── scan: emit `path:count` for every file with >= 1 occurrence of $1 ──────
scan_group() {
    local pattern="$1"
    # `-H` keeps the filename prefix even when xargs hands grep a single file.
    tr '\n' '\0' <"$FILELIST" \
        | xargs -0 grep -oHE "$pattern" 2>/dev/null \
        | cut -d: -f1 \
        | sort \
        | uniq -c \
        | awk '{ print $2 ":" $1 }' \
        | sort \
        || true
}

clean_baseline() {
    sed -e 's/#.*//' -e 's/[[:space:]]*$//' "$1" | grep -v '^$' | sort
}

write_baseline() {
    local file="$1" group="$2" current="$3"
    {
        echo "# ${group} — source-occurrence baseline (path:count)"
        echo "#"
        echo "# Enforced by scripts/check-source-boundary.sh over git-tracked files"
        echo "# under a crate src/ directory — .rs only (vendor/, examples/, docs/, tests/,"
        echo "# excluded)."
        echo "#"
        echo "# This baseline ONLY EVER SHRINKS: each migration PR of"
        echo "# plans/runtime-http-dependency-containment.md deletes lines from it or"
        echo "# lowers counts. A count that grows, or a new file appearing here, fails"
        echo "# CI. A count that drops is reported as a note — shrink the line in the"
        echo "# same PR so the boundary stays honest."
        echo "#"
        echo "# Regenerate with: scripts/check-source-boundary.sh --update"
        echo
        cat "$current"
    } >"$file"
}

FAILED=0

check_group() {
    local group="$1" pattern="$2" baseline="$3"

    local current="$TMPDIR_BOUNDARY/current-$group.txt"
    scan_group "$pattern" >"$current"

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
        echo "  → Go through the r2e-rt / r2e-http facade instead of naming the"
        echo "    dependency directly. The baseline only ever shrinks."
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

check_group "tokio" '\btokio(_util|_stream)?::' "$BOUNDARY_DIR/src-baseline-tokio.txt"
check_group "axum" '\baxum::' "$BOUNDARY_DIR/src-baseline-axum.txt"

if [ "$FAILED" -ne 0 ]; then
    echo "source boundary check FAILED — see plans/runtime-http-dependency-containment.md" >&2
    exit 1
fi

exit 0

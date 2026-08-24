#!/usr/bin/env bash
#
# check-dep-boundary.sh — freeze the *manifest* side of the runtime/HTTP
# dependency boundary (Phase 0 of plans/runtime-http-dependency-containment.md).
#
# For every workspace member (vendored crates excluded), look at its DIRECT
# dependencies of kind "normal" — dev-dependencies and build-dependencies are
# exempt, a test harness may legitimately pull tokio in. Two check groups:
#
#   tokio : tokio, tokio-util, tokio-stream
#   axum  : axum
#
# The resulting crate set is compared against the checked-in allowlists in
# scripts/boundaries/. A crate that gains such a dependency without being
# allowlisted fails the check; an allowlisted crate that no longer has the
# dependency ALSO fails, so the allowlist can only ever shrink honestly.
#
# Usage:
#   scripts/check-dep-boundary.sh            # check (exit 1 on drift)
#   scripts/check-dep-boundary.sh --update   # rewrite the allowlists from the tree
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

command -v jq >/dev/null 2>&1 || {
    echo "error: jq is required by $0" >&2
    exit 2
}

TMPDIR_BOUNDARY=$(mktemp -d)
trap 'rm -rf "$TMPDIR_BOUNDARY"' EXIT

METADATA="$TMPDIR_BOUNDARY/metadata.json"
cargo metadata --format-version 1 --no-deps >"$METADATA"

# ── scan: crates with a direct, non-dev, non-build dependency matching $1 ──
scan_group() {
    local name_regex="$1"
    jq -r --arg re "$name_regex" '
        .packages[]
        | select(.manifest_path | test("/vendor/") | not)
        | . as $pkg
        | .dependencies[]
        | select(.kind == null)          # null = normal; "dev" / "build" exempt
        | select(.name | test($re))
        | $pkg.name
    ' "$METADATA" | sort -u
}

# ── strip comments/blank lines from an allowlist ──────────────────────────
clean_list() {
    sed -e 's/#.*//' -e 's/[[:space:]]*$//' "$1" | grep -v '^$' | sort -u
}

write_allowlist() {
    local file="$1" group="$2" current="$3"
    {
        echo "# ${group} — direct (non-dev) dependency allowlist"
        echo "#"
        echo "# One workspace crate per line. Enforced by scripts/check-dep-boundary.sh."
        echo "# This list ONLY EVER SHRINKS: each migration PR of"
        echo "# plans/runtime-http-dependency-containment.md deletes lines from it."
        echo "# Adding a line is a deliberate, reviewed exception — the check fails"
        echo "# both when an un-listed crate gains the dependency and when a listed"
        echo "# crate no longer has it (stale entry)."
        echo "#"
        echo "# Regenerate with: scripts/check-dep-boundary.sh --update"
        echo
        cat "$current"
    } >"$file"
}

FAILED=0

check_group() {
    local group="$1" name_regex="$2" allowlist="$3"

    local current="$TMPDIR_BOUNDARY/current-$group.txt"
    scan_group "$name_regex" >"$current"

    if [ "$MODE" = "update" ]; then
        write_allowlist "$allowlist" "$group" "$current"
        echo "updated $allowlist ($(wc -l <"$current" | tr -d ' ') crates)"
        return 0
    fi

    if [ ! -f "$allowlist" ]; then
        echo "error: missing allowlist $allowlist (run $0 --update)" >&2
        FAILED=1
        return 0
    fi

    local expected="$TMPDIR_BOUNDARY/expected-$group.txt"
    clean_list "$allowlist" >"$expected"

    local added removed
    added=$(comm -23 "$current" "$expected")
    removed=$(comm -13 "$current" "$expected")

    if [ -n "$added" ]; then
        FAILED=1
        echo "FAIL [$group] crate(s) with a new direct dependency, not in $allowlist:"
        echo "$added" | sed 's/^/  + /'
        echo "  → Route this through the r2e-rt / r2e-http facade instead of taking a"
        echo "    direct dependency. If the dependency is genuinely unavoidable, add the"
        echo "    crate to $allowlist with a justification in the same PR."
        echo
    fi

    if [ -n "$removed" ]; then
        FAILED=1
        echo "FAIL [$group] stale allowlist entry — remove it from $allowlist:"
        echo "$removed" | sed 's/^/  - /'
        echo "  → These crates no longer have the dependency. The allowlist only ever"
        echo "    shrinks: delete the line(s) so the boundary stays honest."
        echo
    fi

    if [ -z "$added" ] && [ -z "$removed" ]; then
        echo "ok   [$group] $(wc -l <"$current" | tr -d ' ') allowlisted crate(s), no drift"
    fi
}

check_group "tokio" '^(tokio|tokio-util|tokio-stream)$' "$BOUNDARY_DIR/dep-allowlist-tokio.txt"
check_group "axum" '^axum$' "$BOUNDARY_DIR/dep-allowlist-axum.txt"

if [ "$FAILED" -ne 0 ]; then
    echo "dependency boundary check FAILED — see plans/runtime-http-dependency-containment.md" >&2
    exit 1
fi

exit 0

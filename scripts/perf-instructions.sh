#!/usr/bin/env bash
# Instruction-count regression gate for the request hot path and the bean
# graph (`r2e-core/benches/instructions.rs`).
#
#   scripts/perf-instructions.sh              # measure the working tree
#   scripts/perf-instructions.sh origin/master  # ... and compare it to a base
#
# With a base ref, the benchmarks first run on a pristine export of that ref
# and are saved as a baseline; the working tree is then measured against it,
# and the script fails if any benchmark executes more than
# PERF_INSTRUCTIONS_LIMIT (default below) more instructions than the base did.
#
# Both sides run the working tree's benchmark file, so a comparison is always
# the same workload on two versions of the framework. When that file does not
# build on the base (the benchmark does not exist there yet, or the pull
# request changes an API it uses), there is nothing to compare: the working
# tree is measured alone and the script says so.
#
# Instruction counts come from Valgrind, not from a clock: the same binary
# gives the same count to within a fraction of a percent on any machine, which
# is what makes a tight limit usable on shared CI runners.
#
# Requires Linux, Valgrind, and a `gungraun-runner` whose version matches the
# `gungraun` entry of Cargo.lock:
#   cargo install gungraun-runner --version <version> --locked
#
# A justified regression is accepted by saying why in the pull request and
# applying the `perf-regression-accepted` label (see .github/workflows/perf.yml).

set -euo pipefail

LIMIT="${PERF_INSTRUCTIONS_LIMIT:-ir=2%}"
PACKAGE=r2e-core
BENCH=instructions
BENCH_FILE="$PACKAGE/benches/$BENCH.rs"
BASELINE=base

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
base_ref="${1:-}"

note() { printf '%s\n' "==> $*" >&2; }
warn() {
    if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
        printf '::warning title=perf-instructions::%s\n' "$*"
    fi
    printf '%s\n' "warning: $*" >&2
}

if [[ "$(uname -s)" != Linux ]]; then
    echo "error: instruction counts need Valgrind, which this gate only supports on Linux" >&2
    exit 2
fi
for tool in valgrind gungraun-runner; do
    if ! command -v "$tool" >/dev/null; then
        echo "error: \`$tool\` not found (see the header of $0)" >&2
        exit 2
    fi
done

target_dir="$(cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
export CARGO_TARGET_DIR="$target_dir"
# One results directory for both sides, wherever each one is built from.
export GUNGRAUN_HOME="$target_dir/gungraun"

bench() {
    cargo bench --locked -p "$PACKAGE" --bench "$BENCH" -- "$@"
}

have_baseline=false
if [[ -n "$base_ref" ]]; then
    base_dir="$(mktemp -d)"
    trap 'rm -rf "$base_dir"' EXIT

    # Stale results must never stand in for a base that failed to build.
    rm -rf "$GUNGRAUN_HOME"

    note "exporting $base_ref"
    git archive "$base_ref" | tar -x -C "$base_dir"

    if ! grep -q "name = \"$BENCH\"" "$base_dir/$PACKAGE/Cargo.toml"; then
        warn "$base_ref has no \`$BENCH\` benchmark: nothing to compare against"
    else
        cp "$BENCH_FILE" "$base_dir/$BENCH_FILE"
        note "measuring $base_ref"
        if (cd "$base_dir" && bench --save-baseline="$BASELINE"); then
            have_baseline=true
        else
            warn "the benchmark does not build or run on $base_ref (did an API it uses change?): no comparison was made, review the numbers below by hand"
            rm -rf "$GUNGRAUN_HOME"
        fi
    fi
fi

if $have_baseline; then
    note "measuring the working tree against $base_ref (limit: $LIMIT)"
    bench --baseline="$BASELINE" --callgrind-limits="$LIMIT"
else
    note "measuring the working tree"
    bench
fi

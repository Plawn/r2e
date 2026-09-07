#!/usr/bin/env bash
#
# publish-crates.sh — publish the workspace to crates.io.
#
# Cargo (>= 1.90) orders a multi-package publish itself: it computes the
# dependency graph, publishes bottom-up, and waits for each crate to appear in
# the index before the crates that need it. So this script is mostly a preflight
# plus the HELD-BACK list below; it does not hand-roll publication waves.
#
# HELD BACK from the release, and why:
#
#   r2e-cli             `src/commands/docs.rs` and `llm_docs.rs` reach outside
#                       the package with include_str!("../../../docs/...") and
#                       ../../../llm/*.md. `cargo package` only packs files
#                       under the crate root, so the verification build fails.
#                       Unblock: vendor those files under r2e-cli/ (synced by
#                       scripts/check-llm-docs.sh) and repoint the includes.
#
# Crates marked `publish = false` (the examples and the test-only crates) are
# skipped by cargo without being named here.
#
# CRATES.IO RATE LIMIT. A *new* crate name costs a token from a bucket that holds
# 5 and refills at 1 per 10 minutes; new versions of an existing crate are a
# different, far looser bucket (1/min, burst 30). This workspace publishes 30 new
# names, so a single run uploads 5 and then takes HTTP 429 on the sixth — that is
# the expected shape of a first release, not a failure. Two ways out:
#
#   * --resume: re-attempt every 10 minutes until the set is closed (~4h for the
#     remaining 25). Each attempt re-derives what is already on crates.io and
#     excludes it, so the script is idempotent and safe to interrupt.
#   * Ask help@crates.io to lift the new-crate limit for this release. Say how
#     many names and why (one workspace, one version). Then a single run finishes.
#
# Usage:
#   scripts/publish-crates.sh --dry-run   # package + verify everything, upload nothing
#   scripts/publish-crates.sh             # one attempt (asks once, then uploads)
#   scripts/publish-crates.sh --resume    # attempt, wait out 429s, repeat until done
#
# Publication is IRREVERSIBLE: a version can be yanked but never replaced, and a
# name is never freed. Run --dry-run first, and read its output.
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HELD_BACK=(r2e-cli)

DRY_RUN=0
RESUME=0
case "${1:-}" in
    --dry-run) DRY_RUN=1 ;;
    --resume)  RESUME=1 ;;
    "")        ;;
    *)         printf 'usage: %s [--dry-run|--resume]\n' "$0" >&2; exit 2 ;;
esac

# Seconds to wait after a 429 before re-attempting. The bucket refills one token
# per 10 minutes; the extra 10s keeps us on the safe side of the clock.
RETRY_SLEEP="${RETRY_SLEEP:-610}"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------

say "Preflight"

[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty — publish from a clean tree"

# The published crates must build for users on stable, not just under the
# nightly pinned by rust-toolchain.toml (that pin exists for trybuild's .stderr
# expectations, and nothing in the tree uses a #![feature] gate).
command -v rustup >/dev/null && rustup toolchain list | grep -q '^stable' \
  || die "no stable toolchain — install one: rustup toolchain install stable"

say "Checking the workspace on stable"
cargo +stable check --workspace --all-targets

# No published crate may depend on a held-back one: cargo would fail late, after
# part of the release is already uploaded and unrecallable.
say "Verifying no published crate depends on a held-back crate"
python3 - "${HELD_BACK[@]}" <<'PY'
import json, subprocess, sys

held = set(sys.argv[1:])
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"]))

bad = []
for pkg in meta["packages"]:
    if pkg.get("publish") == [] or pkg["name"] in held:
        continue
    for dep in pkg["dependencies"]:
        # dev-dependencies without a version are stripped at packaging time, so
        # they cannot break a consumer; everything else ships in the manifest.
        if dep["kind"] == "dev" and dep.get("req") == "*":
            continue
        if dep["name"] in held:
            bad.append(f"{pkg['name']} -> {dep['name']} ({dep['kind'] or 'normal'})")

if bad:
    print("published crates depend on held-back crates:", file=sys.stderr)
    for line in bad:
        print("  " + line, file=sys.stderr)
    sys.exit(1)
print("  ok — the published set is closed")
PY

# ---------------------------------------------------------------------------
# Publish
# ---------------------------------------------------------------------------

version=$(python3 -c "
import json,subprocess
m=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1']))
print(next(p['version'] for p in m['packages'] if p['name']=='r2e'))")

# Which of our names already carry $version on crates.io. Derived from the index
# rather than remembered locally, so a run interrupted anywhere — a 429, a lost
# connection, ^C — resumes from the truth.
published_crates() {
    python3 - "$version" "${HELD_BACK[@]}" <<'PY'
import json, subprocess, sys, urllib.error, urllib.request

version, held = sys.argv[1], set(sys.argv[2:])
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"]))

for pkg in sorted(meta["packages"], key=lambda p: p["name"]):
    name = pkg["name"]
    if pkg.get("publish") == [] or name in held:
        continue
    req = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}",
        headers={"User-Agent": f"r2e-publish-script ({name} {version})"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.load(resp)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:      # name not taken yet
            continue
        raise
    if any(v["num"] == version for v in data.get("versions", [])):
        print(name)
PY
}

# Held back by policy + already uploaded = what this attempt must skip.
REMAINING=0
build_excludes() {
    EXCLUDE_ARGS=()
    local crate
    for crate in "${HELD_BACK[@]}"; do
        EXCLUDE_ARGS+=(--exclude "$crate")
    done

    DONE=()
    while IFS= read -r crate; do
        [[ -n "$crate" ]] || continue
        DONE+=("$crate")
        EXCLUDE_ARGS+=(--exclude "$crate")
    done < <(published_crates)

    REMAINING=$(python3 -c "
import json,subprocess,sys
skip=set(sys.argv[1:])
m=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1']))
print(sum(1 for p in m['packages'] if p.get('publish') != [] and p['name'] not in skip))" \
        "${HELD_BACK[@]}" ${DONE[@]+"${DONE[@]}"})
}

if [[ $DRY_RUN -eq 1 ]]; then
    build_excludes
    say "Dry run — packaging and verifying every crate, uploading nothing"
    cargo +stable publish --workspace "${EXCLUDE_ARGS[@]}" --dry-run
    say "Dry run finished. Re-run without --dry-run to upload."
    exit 0
fi

build_excludes
[[ ${#DONE[@]} -eq 0 ]] || printf '\nAlready on crates.io at %s: %s\n' "$version" "${DONE[*]}"

if [[ $REMAINING -eq 0 ]]; then
    say "Nothing left to publish — every crate is on crates.io at $version"
    exit 0
fi

say "About to publish to crates.io — this cannot be undone"
printf 'Held back: %s\n' "${HELD_BACK[*]}"
printf 'To upload: %s crate(s)\n' "$REMAINING"
read -r -p 'Type the release version to confirm (e.g. 0.3.0): ' answer
[[ "$answer" == "$version" ]] || die "got '$answer', workspace is at '$version' — aborting"

# One attempt. Returns 0 when the whole remaining set went up, 1 when the
# new-crate bucket ran dry (retryable), and dies on anything else — a genuine
# error must not be slept on and retried forever.
attempt() {
    local log status
    log=$(mktemp)
    set +e
    cargo +stable publish --workspace "${EXCLUDE_ARGS[@]}" 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
    set -e
    if [[ $status -eq 0 ]]; then
        rm -f "$log"
        return 0
    fi
    if grep -qiE '429|too many requests|rate limit' "$log"; then
        rm -f "$log"
        return 1
    fi
    rm -f "$log"
    die "publish failed for a reason that is not the rate limit — read the output above"
}

if [[ $RESUME -eq 0 ]]; then
    attempt || die "rate-limited after $REMAINING remaining — re-run with --resume to wait it out"
else
    while :; do
        if attempt; then
            break
        fi
        build_excludes
        say "Rate-limited — $REMAINING crate(s) left, retrying in $((RETRY_SLEEP / 60)) min"
        sleep "$RETRY_SLEEP"
        build_excludes
        [[ $REMAINING -gt 0 ]] || break
    done
fi

say "Published $version"
cat <<'EOF'

Follow-ups:
  * Tag the release and push the tag.
  * r2e-test ships without its dev-dependency on the facade (path-only on
    purpose — see the comment in r2e-test/Cargo.toml). Nothing to do; it is
    stripped at packaging and the tests still run in the workspace.
EOF

#!/usr/bin/env bash
#
# check-release-drift.sh — guard a partially-uploaded release against silent
# divergence.
#
# A published (name, version) pair is IMMUTABLE. crates.io will never let an
# already-uploaded version of a crate be replaced, so once a crate is up, its
# source must stop moving until the release closes. A release that is
# rate-limited (new crate names) spans hours, and normal work continues in the
# tree meanwhile. publish-crates.sh *excludes* what is already up — which is
# what makes it resumable, and also what makes the violation silent: a fix
# landing in an uploaded crate is simply never shipped, and crates.io keeps
# serving the old bytes under a version the repo believes it fixed.
#
# This script names the release point and fails if any crate already on
# crates.io at its current version has moved since it. Crates not yet uploaded
# may change freely — they will be packaged from the tree as it stands when
# their turn comes.
#
# Usage:
#   scripts/check-release-drift.sh [release-ref]
#   default: the last `release: …` commit reachable from HEAD
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

ref="${1:-$(git log -1 --format=%H --grep='^release: ' HEAD)}"
[[ -n "$ref" ]] || die "no 'release: …' commit reachable from HEAD — pass the release ref explicitly"
git rev-parse --verify --quiet "$ref^{commit}" >/dev/null \
  || die "no such ref: $ref — pass the commit the release was cut from"

# name<TAB>version<TAB>directory, for every crate already on crates.io at its
# current version.
uploaded=$(python3 scripts/versions.py uploaded)

if [[ -z "$uploaded" ]]; then
    printf 'Nothing uploaded at the current versions — the tree is free to move.\n'
    exit 0
fi

drifted=()
count=0
while IFS=$'\t' read -r name version dir; do
    [[ -n "$name" ]] || continue
    count=$((count + 1))
    git diff --quiet "$ref" HEAD -- "$dir" || drifted+=("$name $version ($dir)")
done <<< "$uploaded"

if [[ ${#drifted[@]} -gt 0 ]]; then
    printf '\033[31mdrift: %s crate(s) already on crates.io at their current version have changed since %s:\033[0m\n' \
        "${#drifted[@]}" "$ref" >&2
    printf '  %s\n' "${drifted[@]}" >&2
    cat >&2 <<MSG

crates.io cannot be corrected in place. Either revert the change in those crates
until the release closes, or bump them (scripts/bump-version.sh <crate> patch)
and release the new version.
MSG
    exit 1
fi

printf 'ok — all %s uploaded crate(s) are unchanged since %s\n' "$count" "$ref"

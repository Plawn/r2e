#!/usr/bin/env bash
#
# check-release-drift.sh — guard a partially-uploaded release against silent
# divergence.
#
# A published (name, version) pair is IMMUTABLE. crates.io will never let 0.3.0
# of an already-uploaded crate be replaced, so once a crate is up, its source
# must stop moving until the release closes. That is easy to violate here: this
# workspace publishes 30 new names against a bucket that refills one per ten
# minutes, so a first release spans hours, and normal work continues in the tree
# meanwhile. publish-crates.sh *excludes* what is already up — which is what
# makes it resumable, and also what makes the violation silent: a fix landing in
# an uploaded crate is simply never shipped, and crates.io keeps serving the old
# bytes under a version the repo believes it fixed.
#
# This script names the release point and fails if any already-uploaded crate
# has moved since it. Crates not yet uploaded may change freely — they will be
# packaged from the tree as it stands when their turn comes.
#
# Usage:
#   scripts/check-release-drift.sh [release-ref]     # default: tag release/<version>
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HELD_BACK=(r2e-cli)

die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

version=$(python3 -c "
import json,subprocess
m=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1']))
print(next(p['version'] for p in m['packages'] if p['name']=='r2e'))")

ref="${1:-release/$version}"
git rev-parse --verify --quiet "$ref^{commit}" >/dev/null \
  || die "no such ref: $ref — tag the commit the release was cut from, e.g. 'git tag release/$version <sha>'"

# name<TAB>directory, for every crate already on crates.io at $version.
uploaded=$(python3 - "$version" "${HELD_BACK[@]}" <<'PY'
import json, os, subprocess, sys, urllib.error, urllib.request

version, held = sys.argv[1], set(sys.argv[2:])
root = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"]))

for pkg in sorted(meta["packages"], key=lambda p: p["name"]):
    name = pkg["name"]
    if pkg.get("publish") == [] or name in held:
        continue
    req = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}",
        headers={"User-Agent": f"r2e-release-drift ({name} {version})"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.load(resp)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            continue
        raise
    if any(v["num"] == version for v in data.get("versions", [])):
        rel = os.path.relpath(os.path.dirname(pkg["manifest_path"]), root)
        print(f"{name}\t{rel}")
PY
)

if [[ -z "$uploaded" ]]; then
    printf 'Nothing uploaded yet at %s — the tree is free to move.\n' "$version"
    exit 0
fi

drifted=()
count=0
while IFS=$'\t' read -r name dir; do
    [[ -n "$name" ]] || continue
    count=$((count + 1))
    git diff --quiet "$ref" HEAD -- "$dir" || drifted+=("$name ($dir)")
done <<< "$uploaded"

if [[ ${#drifted[@]} -gt 0 ]]; then
    printf '\033[31mdrift: %s crate(s) already on crates.io at %s have changed since %s:\033[0m\n' \
        "${#drifted[@]}" "$version" "$ref" >&2
    printf '  %s\n' "${drifted[@]}" >&2
    cat >&2 <<MSG

crates.io cannot be corrected in place. Either revert the change in those crates
until the release closes, or bump the whole workspace (version.workspace = true
plus the pins in [workspace.dependencies]) and re-release every crate at the new
version.
MSG
    exit 1
fi

printf 'ok — all %s uploaded crate(s) are unchanged since %s\n' "$count" "$ref"

#!/usr/bin/env bash
#
# bump-version.sh — prepare a release: move the whole workspace to a new version.
#
# The version in [workspace.package] is the single source of truth for a
# release. Changing it on master is what makes .github/workflows/release.yml
# publish to crates.io and tag `vX.Y.Z`; nothing else does. This script makes
# that change consistently:
#
#   * [workspace.package] version
#   * every `r2e-* = { path = "…", version = "…" }` pin in [workspace.dependencies]
#     (a pin left behind makes the published crates require the old version)
#   * Cargo.lock (workspace members only)
#   * CHANGELOG.md: `## [Unreleased]` becomes `## [X.Y.Z] - <date>`, a fresh
#     empty `## [Unreleased]` goes on top — that section is the release notes
#   * llm-full.txt (embeds the version) via scripts/check-llm-docs.sh --update
#
# Then commit it on a branch and open a PR titled `release: X.Y.Z`. Merging it
# is the release.
#
# Versioning: pre-1.0, any breaking change bumps the minor (0.3.x -> 0.4.0);
# additions and fixes bump the patch.
#
# Usage:
#   scripts/bump-version.sh 0.4.0
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

[[ $# -eq 1 ]] || { printf 'usage: %s X.Y.Z\n' "$0" >&2; exit 2; }
new="$1"
[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$new' is not X.Y.Z"
[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty — bump from a clean tree"

old=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
[[ -n "$old" ]] || die "could not read [workspace.package] version from Cargo.toml"

python3 - "$old" "$new" <<'PY'
import sys
old, new = sys.argv[1], sys.argv[2]

def key(v):
    return tuple(int(x) for x in v.split("."))

if key(new) <= key(old):
    sys.exit(f"error: {new} is not greater than the current {old}")
PY

# Remote too: a local clone does not necessarily have every tag CI pushed.
if git rev-parse -q --verify "refs/tags/v$new" >/dev/null \
   || [[ -n "$(git ls-remote --tags origin "refs/tags/v$new")" ]]; then
    die "tag v$new already exists — pick a version that was never tagged"
fi

python3 - "$old" "$new" <<'PY'
import datetime, re, sys

old, new = sys.argv[1], sys.argv[2]

# Cargo.toml: the [workspace.package] line and the r2e-* pins.
text = open("Cargo.toml").read()
text, n_pkg = re.subn(rf'(?m)^version = "{re.escape(old)}"$', f'version = "{new}"', text, count=1)
pin = re.compile(rf'(?m)^(r2e[A-Za-z0-9_-]* *= *\{{[^}}\n]*\bversion *= *")({re.escape(old)})(")')
text, n_pins = pin.subn(rf"\g<1>{new}\g<3>", text)
if n_pkg != 1:
    sys.exit("error: [workspace.package] version line not found")
if n_pins == 0:
    sys.exit("error: no r2e-* version pins found in [workspace.dependencies]")
# A pin on another version would silently survive the bump.
stale = re.findall(r'(?m)^(r2e[A-Za-z0-9_-]*) *= *\{[^}\n]*\bversion *= *"([^"]+)"', text)
stale = [f"{name} = {ver}" for name, ver in stale if ver != new]
if stale:
    sys.exit("error: r2e-* pins not on the old version, fix by hand first:\n  " + "\n  ".join(stale))
open("Cargo.toml", "w").write(text)
print(f"Cargo.toml: workspace {old} -> {new}, {n_pins} pin(s)")

# CHANGELOG.md: close the Unreleased section under the new version.
cl = open("CHANGELOG.md").read()
if "## [Unreleased]" not in cl:
    sys.exit("error: CHANGELOG.md has no '## [Unreleased]' section")
today = datetime.date.today().isoformat()
cl = cl.replace("## [Unreleased]", f"## [Unreleased]\n\n## [{new}] - {today}", 1)
open("CHANGELOG.md", "w").write(cl)
print(f"CHANGELOG.md: [Unreleased] -> [{new}] - {today}")
PY

cargo update --workspace --quiet
scripts/check-llm-docs.sh --update >/dev/null

git status --short
cat <<EOF

Bumped $old -> $new. Review CHANGELOG.md's [$new] section (it is the GitHub
release body), then:

  git switch -c release/$new
  git commit -am "release: $new"
  git push -u origin release/$new   # open the PR; merging it publishes $new
EOF

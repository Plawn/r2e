#!/usr/bin/env bash
#
# bump-version.sh — cut a release PR for one or more crates.
#
# Crates are versioned independently. Name the crates that changed and how:
#
#   scripts/bump-version.sh r2e-mcp breaking            # API break in r2e-mcp
#   scripts/bump-version.sh r2e-core patch r2e-mcp minor
#   scripts/bump-version.sh r2e-tenant 0.7.2            # explicit version
#   scripts/bump-version.sh --dry-run r2e-core breaking # plan only
#
# `breaking` is `minor` below 1.0 and `major` from 1.0 on. A breaking bump
# PROPAGATES: every publishable crate that depends on the bumped one (normal or
# build dependency, transitively) is bumped breaking too, because it re-exports
# that crate's types — the facade `r2e` always ends up in that set, and
# `r2e-cli` follows `r2e` version for version. A patch bump propagates nothing.
# `--no-propagate` turns the propagation off (you then own the consequences).
#
# What it does (scripts/versions.py does the editing):
#   * each bumped crate's `[package] version` and its pin in
#     `[workspace.dependencies]`;
#   * each bumped crate's CHANGELOG.md: `[Unreleased]` -> `[X.Y.Z] - date`
#     (a crate bumped by propagation gets a "Built against …" entry; a crate
#     bumped by request must already have entries under `[Unreleased]`);
#   * `R2E_DEP_VERSION` in r2e-cli when `r2e` moves; `Cargo.lock`;
#     `llm-full.txt` (stamped with the facade version);
#   * refuses a version already tagged or already on crates.io.
#
# Then commit on a `release/…` branch and open a PR titled `release: <crates>`.
# Once it is merged and Tests pass, .github/workflows/release.yml publishes the
# bumped crates, tags `<crate>-vX.Y.Z` and creates one GitHub release per crate
# from its CHANGELOG section.
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

DRY_RUN=0
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        -h|--help) sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) ARGS+=("$arg") ;;
    esac
done
[[ ${#ARGS[@]} -gt 0 ]] || die "usage: $0 [--dry-run] [--no-propagate] <crate> <patch|minor|major|breaking|X.Y.Z> […]"

if [[ $DRY_RUN -eq 1 ]]; then
    exec python3 scripts/versions.py plan "${ARGS[@]}"
fi

[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty — commit or stash first"

title=$(python3 scripts/versions.py bump "${ARGS[@]}")

printf '\n==> Updating Cargo.lock\n'
cargo update --workspace --quiet

printf '==> Regenerating llm-full.txt\n'
scripts/check-llm-docs.sh --update >/dev/null

printf '==> Checking the invariants\n'
scripts/check-versions.sh

first=$(python3 - "$title" <<'PY'
import sys; name, ver = sys.argv[1].split(", ")[0].split(" "); print(f"{name}-{ver}")
PY
)

cat <<EOF

Bumped: $title

Review the diff, then:

  git switch -c release/$first
  git commit -am "release: $title"
  git push -u origin release/$first
  gh pr create --title "release: $title" --body "Release PR made by scripts/bump-version.sh."

Merging it (once Tests pass) publishes the bumped crates, tags <crate>-vX.Y.Z and
creates one GitHub release per crate from its CHANGELOG section.
EOF

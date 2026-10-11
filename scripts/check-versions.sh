#!/usr/bin/env bash
#
# check-versions.sh — verify the per-crate versioning invariants.
#
# Crates are versioned independently (no `[workspace.package] version`). This
# checks that every publishable crate declares its own version and changelog,
# that the `[workspace.dependencies]` pins equal the crates' versions, that
# `r2e-cli` tracks `r2e`, and that the derived stamps (`R2E_DEP_VERSION`,
# `llm-full.txt`) match. The rules live in scripts/versions.py; CI runs this on
# every push (.github/workflows/test.yml) and before a release.
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
exec python3 scripts/versions.py check

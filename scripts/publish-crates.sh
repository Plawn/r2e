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
#   r2e-openfga         depends on `openfga-rs` through the `[patch.crates-io]`
#   r2e-openfga-macros  entry pointing at vendor/openfga-rs. A `[patch]` is a
#   r2e-openfga-model   workspace-local construct: it does NOT travel with a
#                       published crate. A consumer enabling this would resolve
#                       the real openfga-rs 0.1.0 (tonic ~0.11), which drags in
#                       axum-core 0.4 next to R2E's 0.5 — the dual axum-core
#                       that vendor/README.md exists to avoid. `model` and
#                       `macros` are themselves clean (a pure .fga parser and a
#                       proc-macro), but shipping them alone would publish a
#                       macro whose generated code names a crate that is not on
#                       crates.io, so the trio ships together or not at all.
#                       Unblock: publish the fork under a name we own, or
#                       generate the gRPC client inside r2e-openfga, then drop
#                       both this exclusion and the [patch.crates-io] section.
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
# Usage:
#   scripts/publish-crates.sh --dry-run   # package + verify everything, upload nothing
#   scripts/publish-crates.sh             # the real thing (asks once, then uploads)
#
# Publication is IRREVERSIBLE: a version can be yanked but never replaced, and a
# name is never freed. Run --dry-run first, and read its output.
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HELD_BACK=(r2e-openfga r2e-openfga-macros r2e-openfga-model r2e-cli)

DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

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

EXCLUDE_ARGS=()
for crate in "${HELD_BACK[@]}"; do
    EXCLUDE_ARGS+=(--exclude "$crate")
done

if [[ $DRY_RUN -eq 1 ]]; then
    say "Dry run — packaging and verifying every crate, uploading nothing"
    cargo +stable publish --workspace "${EXCLUDE_ARGS[@]}" --dry-run
    say "Dry run finished. Re-run without --dry-run to upload."
    exit 0
fi

say "About to publish to crates.io — this cannot be undone"
printf 'Held back: %s\n' "${HELD_BACK[*]}"
read -r -p 'Type the release version to confirm (e.g. 0.3.0): ' answer
version=$(python3 -c "
import json,subprocess
m=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1']))
print(next(p['version'] for p in m['packages'] if p['name']=='r2e'))")
[[ "$answer" == "$version" ]] || die "got '$answer', workspace is at '$version' — aborting"

cargo +stable publish --workspace "${EXCLUDE_ARGS[@]}"

say "Published $version"
cat <<'EOF'

Follow-ups:
  * Tag the release and push the tag.
  * r2e-test ships without its dev-dependency on the facade (path-only on
    purpose — see the comment in r2e-test/Cargo.toml). Nothing to do; it is
    stripped at packaging and the tests still run in the workspace.
  * The facade's `openfga` feature is commented out in r2e/Cargo.toml. Restore
    it, the optional dependency, and the two re-exports in r2e/src/lib.rs in the
    same change that unblocks r2e-openfga.
EOF

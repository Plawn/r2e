#!/usr/bin/env bash
# Copy the workspace LICENSE into every publishable crate root.
#
# `cargo package` packs only what lives under the crate root, so a single
# LICENSE at the workspace root ships in no .crate at all. Apache-2.0 §4(a)
# wants the License to reach every recipient of a distribution, and a .crate is
# a distribution — so each crate carries its own copy, the same choice tokio
# and serde make.
#
# Run this after adding a crate; `scripts/publish-crates.sh` verifies the copies
# and refuses to publish when one is missing or has drifted.
set -euo pipefail

cd "$(dirname "$0")/.."

python3 - <<'INNER'
import json, os, shutil, subprocess

meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"]))
root = meta["workspace_root"]
src = os.path.join(root, "LICENSE")

n = 0
for pkg in meta["packages"]:
    if pkg.get("publish") == []:
        continue
    shutil.copyfile(src, os.path.join(os.path.dirname(pkg["manifest_path"]), "LICENSE"))
    n += 1
print(f"synced LICENSE into {n} crate roots")
INNER

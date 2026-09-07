#!/usr/bin/env bash
#
# generate-openfga-proto.sh — regenerate the checked-in OpenFGA gRPC client.
#
# `r2e-openfga` has no build.rs on purpose: a build script would make `protoc` a
# hard requirement for every consumer that merely enables the `openfga` feature,
# even though such a consumer never authors a proto. The generated client is
# committed under r2e-openfga/src/proto/ instead, and this script is the only
# thing that needs protoc.
#
#   ./scripts/generate-openfga-proto.sh           regenerate in place
#   ./scripts/generate-openfga-proto.sh --check   fail if the tree is stale (CI)
#
set -euo pipefail

cd "$(dirname "$0")/.."

GENERATED="r2e-openfga/src/proto/openfga.v1.rs"
CHECK=0
[[ "${1:-}" == "--check" ]] && CHECK=1

if ! command -v protoc >/dev/null 2>&1; then
  echo "error: protoc not found. Install it (brew install protobuf) — only this" >&2
  echo "       script needs it, never a consumer of the published crate." >&2
  exit 1
fi

if [[ $CHECK -eq 1 ]]; then
  # Check mode must never leave a mark on the tree, whatever happens: the
  # generator can die *after* rewriting the file (a failed prune, a killed
  # build), and `set -e` would then skip any restore placed further down. The
  # trap restores the committed file on every exit path, success included —
  # a matching regeneration is byte-identical, so restoring is a no-op there.
  before=$(mktemp)
  cp "$GENERATED" "$before"
  restore() {
    cp "$before" "$GENERATED"
    rm -f "$before"
    # The generator prunes the modules prost emits for the annotation-only
    # protos (google.api, validate, openapiv2) as its last step; if it died
    # before that, drop them here so a failed check leaves nothing behind.
    find "$(dirname "$GENERATED")" -type f ! -name "$(basename "$GENERATED")" -delete
  }
  trap restore EXIT

  cargo run -q -p r2e-openfga-codegen >/dev/null

  if ! diff -q "$before" "$GENERATED" >/dev/null; then
    echo "error: $GENERATED is out of sync with r2e-openfga/proto/." >&2
    echo "       Run ./scripts/generate-openfga-proto.sh and commit the result." >&2
    diff -u "$before" "$GENERATED" | head -50 >&2 || true
    exit 1
  fi
  echo "$GENERATED is up to date."
else
  cargo run -q -p r2e-openfga-codegen
  echo "Regenerated $GENERATED — commit it."
fi

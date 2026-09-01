#!/usr/bin/env bash
#
# check-llm-docs.sh — validate the AI/agent-facing reference and keep its
# generated concatenation in sync (plans/llm-docs-split.md).
#
# Layout:
#   llm.txt          hand-written HUB: what R2E is, the golden rules and the
#                    routing table "task → llm/<topic>.md". It is also the
#                    MANIFEST: every spoke must be referenced from it (that is
#                    what makes the routing table complete), and the order of
#                    first reference is the concatenation order.
#   llm/<topic>.md   one SPOKE per topic: a front-matter block, exactly one
#                    `## Title`, a `### TL;DR` subsection, then the content.
#   llm-full.txt     GENERATED: hub + every spoke (front matter stripped) in
#                    manifest order, for tools that ingest one file. Never
#                    edited by hand.
#
# Spoke front matter (exact key order, nothing else):
#   ---
#   topic: <slug>            must equal the file name
#   features: <r2e features to enable, or `core`>
#   tokens: ~<N>             recomputed here (bytes/4, rounded to 100)
#   requires: <slugs>        prerequisite spokes, comma-separated, may be empty
#   ---
#
# Usage:
#   scripts/check-llm-docs.sh            # check (exit 1 on any drift)
#   scripts/check-llm-docs.sh --update   # rewrite tokens: + llm-full.txt
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HUB="llm.txt"
SRC_DIR="llm"
FULL="llm-full.txt"
MODE="check"
if [ "${1:-}" = "--update" ]; then
    MODE="update"
elif [ -n "${1:-}" ]; then
    echo "usage: $0 [--update]" >&2
    exit 2
fi

status=0
fail() { echo "error: $*" >&2; status=1; }

VERSION=$(grep -m1 -E '^version = "' Cargo.toml | sed -E 's/^version = "([^"]+)"/\1/')
[ -n "$VERSION" ] || { echo "error: cannot read workspace version from Cargo.toml" >&2; exit 2; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# ── manifest: spokes in order of first reference from the hub ─────────────
grep -oE "$SRC_DIR/[a-z0-9-]+\.md" "$HUB" | awk '!seen[$0]++' >"$TMP/manifest"
ls "$SRC_DIR"/*.md | sort >"$TMP/on-disk"
sort "$TMP/manifest" >"$TMP/manifest.sorted"

comm -13 "$TMP/manifest.sorted" "$TMP/on-disk" | while read -r f; do
    echo "error: $f is not routed from $HUB — add a row to the routing table" >&2
done
comm -23 "$TMP/manifest.sorted" "$TMP/on-disk" | while read -r f; do
    echo "error: $HUB references $f, which does not exist" >&2
done
if ! cmp -s "$TMP/manifest.sorted" "$TMP/on-disk"; then status=1; fi

# ── per-spoke structure ───────────────────────────────────────────────────
for f in "$SRC_DIR"/*.md; do
    slug=$(basename "$f" .md)
    # bash 3.2 (macOS) has no mapfile — read the six front-matter lines one by one
    l1=$(sed -n '1p' "$f"); l2=$(sed -n '2p' "$f"); l3=$(sed -n '3p' "$f")
    l4=$(sed -n '4p' "$f"); l5=$(sed -n '5p' "$f"); l6=$(sed -n '6p' "$f")
    [ "$l1" = "---" ] || fail "$f: line 1 must be '---' (front matter)"
    [ "$l2" = "topic: $slug" ] || fail "$f: line 2 must be 'topic: $slug'"
    [[ "$l3" =~ ^features:\ .+ ]] || fail "$f: line 3 must be 'features: <…>'"
    [[ "$l4" =~ ^tokens:\ ~[0-9]+$ ]] || fail "$f: line 4 must be 'tokens: ~<N>'"
    [[ "$l5" =~ ^requires:(\ .*)?$ ]] || fail "$f: line 5 must be 'requires: <slugs>'"
    [ "$l6" = "---" ] || fail "$f: line 6 must be '---' (end of front matter)"

    # prerequisites must be spokes
    reqs=$(sed -n '5s/^requires: *//p' "$f" | tr ',' ' ')
    for r in $reqs; do
        [ -f "$SRC_DIR/$r.md" ] || fail "$f: requires '$r' but $SRC_DIR/$r.md does not exist"
        [ "$r" != "$slug" ] || fail "$f: requires itself"
    done

    tail -n +7 "$f" >"$TMP/body"
    [ "$(sed -n '1p' "$TMP/body")" = "" ] || fail "$f: line 7 must be blank"
    [[ "$(sed -n '2p' "$TMP/body")" =~ ^##\  ]] || fail "$f: line 8 must be the single '## Title' heading"

    extra=$(awk 'NR > 2 && /^```/ { fence = !fence } NR > 2 && !fence && /^## / { print NR + 6 ": " $0 }' "$TMP/body")
    [ -z "$extra" ] || fail "$f: more than one '## ' section — one topic per file:"$'\n'"$(echo "$extra" | sed 's/^/    /')"

    grep -qE '^### TL;DR$' "$TMP/body" || fail "$f: missing '### TL;DR' subsection"

    # tokens: bytes/4 rounded to the nearest 100
    bytes=$(wc -c <"$TMP/body" | tr -d ' ')
    want=$(( (bytes / 4 + 50) / 100 * 100 ))
    have=$(sed -n '4s/^tokens: ~//p' "$f")
    if [ "$have" != "$want" ]; then
        if [ "$MODE" = "update" ]; then
            sed -i.bak "4s/^tokens: ~.*/tokens: ~$want/" "$f" && rm -f "$f.bak"
            echo "updated $f: tokens ~$have → ~$want"
        else
            fail "$f: tokens is ~$have, should be ~$want — run: $0 --update"
        fi
    fi
done

# ── llm-full.txt ──────────────────────────────────────────────────────────
{
    echo "<!-- R2E v$VERSION — generated from $HUB + $SRC_DIR/*.md by scripts/check-llm-docs.sh; do not edit. -->"
    echo
    cat "$HUB"
    while read -r f; do
        [ -f "$f" ] || continue
        printf '\n---\n\n'
        tail -n +8 "$f"
    done <"$TMP/manifest"
} >"$TMP/full"

if [ "$MODE" = "update" ]; then
    cp "$TMP/full" "$FULL"
    echo "regenerated $FULL (v$VERSION, $(wc -l <"$FULL" | tr -d ' ') lines)"
elif ! cmp -s "$TMP/full" "$FULL"; then
    fail "$FULL is out of date — run: $0 --update"
    diff -u "$FULL" "$TMP/full" | head -n 40 >&2 || true
fi

if [ $status = 0 ]; then
    echo "llm docs: OK ($(wc -l <"$TMP/manifest" | tr -d ' ') spokes routed from $HUB, $FULL current, v$VERSION)"
fi
exit $status

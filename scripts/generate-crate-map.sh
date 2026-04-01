#!/usr/bin/env bash
# Generate the crate map for docs/book/src/reference/crate-map.md
# Usage: ./scripts/generate-crate-map.sh > docs/book/src/reference/crate-map.md
#
# Reads Cargo.toml files across the workspace to produce an up-to-date
# crate listing, dependency flow, and feature flags table.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

FACADE_TOML="r2e/Cargo.toml"

# ── helpers ──────────────────────────────────────────────────────────────

get_desc() {
    local desc
    desc=$(grep '^description' "$1" 2>/dev/null | head -1 | sed 's/^description *= *"//; s/"$//')
    echo "$desc"
}

# ── crate list ───────────────────────────────────────────────────────────

collect_crates() {
    echo "r2e"
    for dir in r2e-*/; do
        dir="${dir%/}"
        [ -f "$dir/Cargo.toml" ] || continue
        echo "$dir"
    done
}

# ── output ───────────────────────────────────────────────────────────────

cat <<'HEADER'
# Crate Map

R2E is organized as a workspace of focused crates. The `r2e` facade crate re-exports everything with feature gates.

## Crate overview

| Crate | Description |
|-------|-------------|
HEADER

collect_crates | while IFS= read -r name; do
    toml="$name/Cargo.toml"
    desc=$(get_desc "$toml")
    echo "| \`$name\` | $desc |"
done

cat <<'MID'

## Dependency flow

```
r2e-http (HTTP abstraction - sole axum dependency)
    ^
r2e-macros (proc-macro, no runtime deps)
    ^
r2e-core (runtime foundation, re-exports r2e-http as `http` module)
    ^
r2e-security / r2e-events / r2e-scheduler / r2e-data / r2e-grpc
    ^
r2e-data-sqlx / r2e-cache / r2e-rate-limit / r2e-openapi / r2e-utils
r2e-prometheus / r2e-observability / r2e-oidc / r2e-openfga / r2e-static
r2e-events-iggy / r2e-events-kafka / r2e-events-pulsar / r2e-events-rabbitmq
r2e-devtools / r2e-test
    ^
r2e (facade)
    ^
your application
```

## Feature flags

The `r2e` facade crate gates sub-crates behind features.

**Default features:** `security`, `events`, `utils`

| Feature | Crates / effect |
|---------|----------------|
MID

# Parse features from Cargo.toml (skip default and full lines)
awk '
    /^\[features\]/ { in_feat=1; next }
    /^\[/           { in_feat=0 }
    in_feat && /^[a-z]/ && !/^default/ && !/^full/ {
        # grab everything after " = "
        sub(/^[^ ]+ *= */, "")
        feat = $0
        # get the feature name from the original line
    }
' "$FACADE_TOML" > /dev/null  # dummy, we'll use grep instead

# Simpler approach: parse each feature line
grep -E '^[a-z]' "$FACADE_TOML" | grep -v '^\[' | grep ' = ' | while IFS= read -r line; do
    feat=$(echo "$line" | sed 's/ *=.*//')
    val=$(echo "$line" | sed 's/^[^=]*= *//')

    # skip default and full
    case "$feat" in
        default|full) continue ;;
    esac

    # Clean up the value for display
    clean=$(echo "$val" | sed 's/\[//g; s/\]//g; s/"//g; s/dep://g; s/,/, /g' | sed 's/  */ /g')
    echo "| \`$feat\` | $clean |"
done

echo '| `full` | All of the above (except `dev-reload`) |'

cat <<'FOOTER'

## Using sub-crates directly

While most applications should use the `r2e` facade, you can depend on individual crates:

```toml
[dependencies]
r2e-core = "0.1"
r2e-macros = "0.1"
r2e-security = "0.1"
```

The proc macros use `proc-macro-crate` for dynamic path detection — they check for `r2e` first, then fall back to `r2e-core`. This means generated code uses `::r2e::` paths when using the facade, or `::r2e_core::` when using crates directly.
FOOTER

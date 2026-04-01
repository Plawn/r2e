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
    local d
    d=$(grep '^description' "$1" 2>/dev/null | head -1 | sed 's/^description *= *"//; s/"$//')
    echo "${d:-(no description)}"
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

# ── extract [features] section only ──────────────────────────────────────

extract_features() {
    awk '
        /^\[features\]/ { inside=1; next }
        /^\[/           { inside=0 }
        inside && /^[a-z]/ && !/^default / && !/^full / {
            # Split on first " = "
            idx = index($0, " = ")
            if (idx == 0) next
            feat = substr($0, 1, idx - 1)
            val  = substr($0, idx + 3)
            # Clean up val: remove brackets, quotes, "dep:" prefix
            gsub(/[\[\]"]/, "", val)
            gsub(/dep:/, "", val)
            gsub(/, */, ", ", val)
            print feat "|" val
        }
    ' "$FACADE_TOML"
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
    desc=$(get_desc "$name/Cargo.toml")
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

extract_features | while IFS='|' read -r feat val; do
    echo "| \`$feat\` | $val |"
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

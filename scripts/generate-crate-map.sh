#!/usr/bin/env bash
# Generate the crate map for docs/book/src/reference/crate-map.md
# Usage: ./scripts/generate-crate-map.sh > docs/book/src/reference/crate-map.md
#
# Reads Cargo.toml files across the workspace to produce an up-to-date
# crate listing (every publishable workspace member, with its current
# version), the dependency flow, and the facade feature-flag table (with
# `full` membership computed from `r2e/Cargo.toml`).
#
# Portable to the macOS system bash (3.2): no associative arrays.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

FACADE_TOML="r2e/Cargo.toml"

# ── helpers ──────────────────────────────────────────────────────────────

# First value of a top-level `key = "..."` line of a manifest.
get_field() {
    local d
    d=$(grep "^$2 *= *\"" "$1" 2>/dev/null | head -1 | sed "s/^$2 *= *\"//; s/\".*\$//")
    echo "$d"
}

# `0.15.0` -> `0.15` (the requirement users write).
major_minor() {
    echo "$1" | awk -F. '{ print $1 "." $2 }'
}

is_publishable() {
    ! grep -q '^publish *= *false' "$1"
}

# Workspace members, in manifest order, from the root Cargo.toml.
workspace_members() {
    awk '
        /^members *= *\[/ { inside=1; next }
        inside && /^\]/   { inside=0 }
        inside {
            gsub(/[",]/, "")
            gsub(/^[ \t]+|[ \t]+$/, "")
            if ($0 != "") print
        }
    ' Cargo.toml
}

# `name|version|path` for every publishable member, facade first, then by name.
collect_crates() {
    workspace_members | while IFS= read -r dir; do
        local toml="$dir/Cargo.toml"
        [ -f "$toml" ] || continue
        is_publishable "$toml" || continue
        echo "$(get_field "$toml" name)|$(get_field "$toml" version)|$dir"
    done | awk -F'|' '{ print ($1 == "r2e" ? "0" : "1") "|" $0 }' | sort -t'|' -k1,1 -k2,2 | cut -d'|' -f2-
}

crate_version() {
    get_field "$1/Cargo.toml" version
}

# The `full = [...]` list of the facade, one feature per line.
full_features() {
    grep '^full *= *\[' "$FACADE_TOML" | sed 's/^full *= *\[//; s/\].*$//' | tr ',' '\n' | tr -d ' "' | grep -v '^$'
}

# `feature|value` for every facade feature except `default` and `full`.
extract_features() {
    awk '
        /^\[features\]/ { inside=1; next }
        /^\[/           { inside=0 }
        inside && /^[a-z]/ && !/^default / && !/^full / {
            idx = index($0, " = ")
            if (idx == 0) next
            feat = substr($0, 1, idx - 1)
            val  = substr($0, idx + 3)
            gsub(/[\[\]"]/, "", val)
            gsub(/dep:/, "", val)
            gsub(/, */, ", ", val)
            if (val == "") val = "(nothing)"
            print feat "|" val
        }
    ' "$FACADE_TOML"
}

# Human-facing note per facade feature. A feature missing here still gets a
# row (with an empty note), so a new feature never silently drops out.
feature_note() {
    case "$1" in
        security)        echo 'JWT/JWKS validation, `AuthenticatedUser`, roles (default)' ;;
        events)          echo 'In-process `EventBus` (default)' ;;
        utils)           echo '`Logged`, `Timed`, `Cache`, `CacheInvalidate` interceptors (default)' ;;
        data)            echo 'No-op compatibility marker: pagination (`Page`/`Pageable`) is always in core' ;;
        data-sqlx)       echo 'Managed SQLx transactions + `DataSource` plugin (pick a driver below)' ;;
        data-diesel)     echo 'Managed Diesel transactions + `DataSource` plugin (pick a driver below)' ;;
        sqlx-*)          echo 'SQLx backend with this driver' ;;
        diesel-*)        echo 'Diesel backend with this driver' ;;
        tenant-sqlx)     echo 'Per-tenant SQLx pools / `TenantTx`' ;;
        tenant-diesel)   echo 'Per-tenant Diesel pools / `TenantTx`' ;;
        sqlite|postgres|mysql) echo 'Compatibility alias; prefer the `sqlx-*` feature' ;;
        events-*)        echo 'Distributed event bus backend' ;;
        scheduler)       echo 'Interval/cron `#[scheduled]` tasks (pulls in `executor`)' ;;
        scheduler-testing) echo 'Scheduler test helpers; enable from `[dev-dependencies]` only' ;;
        executor)        echo 'Managed task pool, `#[async_exec]`, background services' ;;
        cache)           echo '`TtlCache` + pluggable `CacheStore`' ;;
        rate-limit)      echo 'Token-bucket rate limiting' ;;
        oidc)            echo 'Embedded OAuth/JWT issuer' ;;
        openapi)         echo 'OpenAPI 3.1 generation + Swagger UI' ;;
        prometheus)      echo 'HTTP metrics + `/metrics` endpoint' ;;
        metrics-facade)  echo 'HTTP metrics through the `metrics` crate (`MetricsFacade`); a stack choice' ;;
        openfga)         echo 'OpenFGA authorization (`model!`, `FgaCheck` guards)' ;;
        observability)   echo 'OpenTelemetry tracing + OTLP export' ;;
        grpc)            echo 'Tonic gRPC services' ;;
        grpc-reflection) echo 'gRPC server reflection' ;;
        grpc-web)        echo 'gRPC-Web' ;;
        mcp)             echo 'MCP server (`#[tool]`/`#[resource]`/`#[prompt]`, `McpServer`)' ;;
        mcp-testing)     echo 'MCP test helpers; enable from `[dev-dependencies]` only' ;;
        mcp-loopback)    echo 'In-process MCP sessions (`McpLoopback`)' ;;
        static)          echo 'Embedded static files / SPA' ;;
        tenant)          echo 'Multi-tenant bean routing (`Tenanted<T>`, `Tenancy`)' ;;
        ws)              echo 'WebSockets' ;;
        multipart)       echo 'Multipart form uploads' ;;
        json-sonic)      echo 'SIMD JSON codec (sonic-rs) behind `r2e::json`' ;;
        quic)            echo 'HTTP/3 over QUIC (heavy crypto deps)' ;;
        dev-reload)      echo 'Subsecond hot-reload for `r2e dev`; never in production builds' ;;
        test-util)       echo 'Paused clocks for `#[r2e::test(start_paused = true)]`' ;;
        *)               echo '' ;;
    esac
}

# ── output ───────────────────────────────────────────────────────────────

FULL_LIST=$(full_features)

in_full() {
    echo "$FULL_LIST" | grep -qx "$1"
}

cat <<'HEADER'
<!-- Generated by scripts/generate-crate-map.sh — edit the script, not this file. -->

# Crate Map

R2E is organized as a workspace of focused crates. The `r2e` facade crate re-exports everything with feature gates.

## Crate overview

Every publishable crate, with its current version. Crates are versioned independently;
each one has its own `CHANGELOG.md`.

| Crate | Version | Path | Description |
|-------|---------|------|-------------|
HEADER

collect_crates | while IFS='|' read -r name version dir; do
    desc=$(get_field "$dir/Cargo.toml" description)
    echo "| \`$name\` | $version | \`$dir\` | ${desc:-(no description)} |"
done

cat <<'MID'

## Dependency flow

```
r2e-rt (async-runtime facade - sole tokio dependency)
r2e-tracing (process-global tracing subscriber - no tokio, no r2e deps)
    ^
r2e-http (HTTP abstraction - sole axum dependency)
    ^
r2e-macros (proc-macro, no runtime deps)
    ^
r2e-core (runtime foundation, re-exports r2e-http as `http` module)
    ^
r2e-security / r2e-events / r2e-executor / r2e-scheduler / r2e-grpc / r2e-tenant
    ^
r2e-mcp / r2e-data-sqlx / r2e-data-diesel / r2e-cache / r2e-rate-limit / r2e-openapi / r2e-utils
r2e-prometheus / r2e-observability / r2e-oidc / r2e-openfga / r2e-static
r2e-events-iggy / r2e-events-kafka / r2e-events-pulsar / r2e-events-rabbitmq
r2e-devtools / r2e-devservices / r2e-test
    ^
r2e (facade)
    ^
your application
```

> `r2e-grpc-build` is a build-dependency (build-script helper), not part of the
> runtime dependency graph. `r2e-openfga-model` and `r2e-openfga-macros` are
> internal to `r2e-openfga`. Event bus backends live under `r2e-events/backends/*`
> and the data backends under `r2e-data/backends/*`.

## Feature flags

The `r2e` facade crate gates sub-crates behind features.

MID

default_feats=$(grep '^default *= *\[' "$FACADE_TOML" | sed 's/^default *= *\[//; s/\].*$//' | tr -d ' "' | sed 's/,/`, `/g')
echo "**Default features:** \`$default_feats\`"
echo
echo '| Feature | Enables | In `full` | Notes |'
echo '|---------|---------|-----------|-------|'

extract_features | while IFS='|' read -r feat val; do
    if in_full "$feat"; then mark="yes"; else mark="no"; fi
    echo "| \`$feat\` | $val | $mark | $(feature_note "$feat") |"
done

full_inline=$(echo "$FULL_LIST" | sed 's/^/`/; s/$/`/' | paste -sd, - | sed 's/,/, /g')
not_full=$(extract_features | cut -d'|' -f1 | while IFS= read -r f; do in_full "$f" || echo "$f"; done \
    | sed 's/^/`/; s/$/`/' | paste -sd, - | sed 's/,/, /g')

cat <<EOF

**\`full\`** enables exactly: $full_inline.

Everything else is opt-in and deliberately left out of \`full\`: $not_full.
Database backends and drivers, distributed event-bus backends and OpenFGA are a
deployment choice; \`quic\` pulls heavy crypto dependencies; \`dev-reload\` and
\`test-util\` must never reach a production build; the \`*-testing\` features belong in
\`[dev-dependencies]\`.

## Using sub-crates directly

While most applications should use the \`r2e\` facade, you can depend on individual crates:

\`\`\`toml
[dependencies]
r2e-core = "$(major_minor "$(crate_version r2e-core)")"
r2e-macros = "$(major_minor "$(crate_version r2e-macros)")"
r2e-security = "$(major_minor "$(crate_version r2e-security)")"
\`\`\`

Because crates are versioned independently, the numbers differ from the facade's
(\`r2e = "$(major_minor "$(crate_version r2e)")"\`). The proc macros use \`proc-macro-crate\` for dynamic path detection — they check for \`r2e\` first, then fall back to \`r2e-core\`. This means generated code uses \`::r2e::\` paths when using the facade, or \`::r2e_core::\` when using crates directly.
EOF

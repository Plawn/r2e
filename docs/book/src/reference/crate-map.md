# Crate Map

R2E is organized as a workspace of focused crates. The `r2e` facade crate re-exports everything with feature gates.

## Crate overview

```
r2e              Facade crate — re-exports everything, feature-gated
r2e-core         Runtime: AppBuilder, Controller, guards, interceptors, config, plugins
r2e-macros       Proc macros: #[derive(Controller)], #[routes], #[bean], #[producer]
r2e-security     JWT/OIDC: AuthenticatedUser, JwtValidator, JWKS cache
r2e-events       In-process typed EventBus with pub/sub
r2e-scheduler    Background task scheduling (interval, cron)
r2e-data         Database: Entity, Repository, QueryBuilder, Pageable/Page
r2e-data-sqlx    SQLx backend: SqlxRepository, Tx, HasPool, migrations
r2e-data-diesel  Diesel backend (skeleton)
r2e-cache        TTL cache with pluggable backends
r2e-rate-limit   Token-bucket rate limiting with pluggable backends
r2e-openapi      OpenAPI 3.0.3 spec generation + docs UI
r2e-prometheus   Prometheus metrics middleware
r2e-observability Structured observability (tracing, metrics)
r2e-openfga      OpenFGA authorization integration
r2e-oidc         Embedded OIDC server — issue JWT tokens without an external IdP
r2e-utils        Built-in interceptors: Logged, Timed, Cache, CacheInvalidate
r2e-devtools     Subsecond hot-reload support (wraps dioxus-devtools)
r2e-test         TestApp, TestJwt for integration testing
r2e-cli          CLI scaffolding tool
```

## Dependency flow

```
r2e-macros (proc-macro, no runtime deps)
    ↑
r2e-core (runtime foundation)
    ↑
r2e-security / r2e-events / r2e-scheduler / r2e-data
    ↑
r2e-data-sqlx / r2e-cache / r2e-rate-limit / r2e-openapi / r2e-utils / r2e-devtools / r2e-test
    ↑
r2e (facade)
    ↑
your application
```

## Feature flags

The `r2e` facade crate gates sub-crates behind features:

| Feature | Crates enabled |
|---------|---------------|
| `security` | `r2e-security` |
| `events` | `r2e-events` |
| `scheduler` | `r2e-scheduler` |
| `data` | `r2e-data`, `r2e-data-sqlx` |
| `cache` | `r2e-cache` |
| `rate-limit` | `r2e-rate-limit` |
| `openapi` | `r2e-openapi` |
| `utils` | `r2e-utils` |
| `oidc` | `r2e-oidc` |
| `prometheus` | `r2e-prometheus` |
| `dev-reload` | `r2e-devtools` (Subsecond hot-patch, **not** in `full`) |
| `full` | All of the above (except `dev-reload`) |

## Using sub-crates directly

While most applications should use the `r2e` facade, you can depend on individual crates:

```toml
[dependencies]
r2e-core = "0.1"
r2e-macros = "0.1"
r2e-security = "0.1"
```

The proc macros use `proc-macro-crate` for dynamic path detection — they check for `r2e` first, then fall back to `r2e-core`. This means generated code uses `::r2e::` paths when using the facade, or `::r2e_core::` when using crates directly.

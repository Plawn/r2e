# r2e-tenant

Multi-tenant bean routing for R2E — one process, many tenants, each with its own database pool, API client, cache namespace or feature set.

## Overview

Per-tenant resources are resolved from the request, created on first use and reclaimed when idle. There are three moving parts:

| Piece | What it is | Who writes it |
|-------|------------|---------------|
| `TenantResolver` | request → `TenantId` | you, or a built-in like `HeaderTenantResolver` |
| `TenantSource<T>` | tenant → `T` | you |
| `Tenanted<T>` | the map holding every tenant's `T` | the framework |

Wiring is two plugins; using it is one request-scoped field, and the handler never names a tenant. Forgetting a plugin is a **compile error** at controller registration, not a 500 on the first request in production.

`Tenanted<T>` single-flights concurrent first requests for a tenant, does not cache failures, bounds creation with a timeout, evicts and disposes idle resources, and drains them at shutdown.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.15", features = ["tenant"] }
# or "tenant-sqlx" / "tenant-diesel" for ready-made per-tenant pools and #[managed] transactions
```

The crate is reachable as `r2e::tenant`.

### Wiring

```rust
use r2e::prelude::*;
use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy, Tenant};

AppBuilder::new()
    // 1. how a request names its tenant
    .provide(HeaderTenantResolver::default())          // x-tenant-id
    .plugin(Tenancy::resolver::<HeaderTenantResolver>())
    // 2. how a tenant gets its pool
    .provide(TenantPools::new(directory))              // impl TenantSource<PgPool>
    .plugin(PerTenant::<PgPool>::from::<TenantPools>().max_active(200))
    // ...
```

### Using a per-tenant resource

```rust
#[controller(path = "/orders")]
struct OrderController {
    #[inject(request)]
    db: Tenant<PgPool>,   // this request's tenant's pool
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> Result<Json<Vec<Order>>, HttpError> {
        Ok(Json(
            sqlx::query_as("select * from orders")
                .fetch_all(&*self.db)
                .await?,
        ))
    }
}
```

`Option<Tenant<T>>` / `Option<TenantId>` mean "no tenant here". A `TenantSource` receives a `TenantContext`, so a per-tenant API client can be built on the same tenant's per-tenant pool (`TenantContext::get`), with cycle detection.

## Configuration

```yaml
tenancy:
  enabled: true          # false → the app boots, nothing resolves
  on-missing: reject     # or `allow` (Option extractors see None)
  max-active: 500        # live per-tenant resources, LRU beyond
  idle-ttl: 15m          # evict + dispose after this much idleness
  create-timeout: 10s    # per `create` call; blowing it is a 504
  negative-ttl: 5s       # how long an unknown tenant is remembered
```

Per-resource overrides go on the `PerTenant` builder.

## Failure mapping

`TenantError` has one status per failure mode: 400 missing, 404 unknown, 503 unavailable, 504 timeout, 500 for wiring bugs. The three request-driven ones are configurable.

## Key types

| Type | Description |
|------|-------------|
| `Tenancy` | Plugin installing the resolver and the per-request resolve-once cell |
| `PerTenant<T>` | Plugin registering a `Tenanted<T>` map fed by a `TenantSource<T>` |
| `Tenant<T>` | Request-scoped extractor: the current tenant's `T` |
| `TenantId` | Validated tenant identifier (also a request-scoped extractor) |
| `TenantResolver` / `SyncTenantResolver` | Request → tenant resolution (built-ins for headers, path parameters, request extensions and closures) |
| `TenantSource<T>` / `TenantContext` | How a tenant's resource is created |
| `TenantRouter` | Bean resolving the tenant of a request according to the deployment policy |
| `TenancyConfig` | Typed `tenancy.*` section |
| `TenantError` | Failure type with per-mode HTTP statuses |

## Documentation

Full guide: [`docs/features/24-tenancy.md`](../docs/features/24-tenancy.md).

## License

Apache-2.0

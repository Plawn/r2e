# Multi-Tenancy

The `r2e-tenant` crate routes each request to a per-tenant copy of a bean: a
database pool, a client, branding. A **resolver** reads the tenant from the
request, a **source** builds the tenant's bean on first use, and the framework
keeps the live copies in a bounded map.

## Setup

```toml
r2e = { version = "0.15", features = ["tenant"] }                        # the layer itself
r2e = { version = "0.15", features = ["tenant-sqlx", "sqlx-postgres"] }  # + per-tenant SQLx pools
r2e = { version = "0.15", features = ["tenant-diesel", "diesel-postgres"] }
```

`tenant` is part of `full`; `tenant-sqlx` and `tenant-diesel` are not, because
they pull in a database backend. The crate is re-exported as `r2e::tenant`.

## The three pieces

| Piece | What it does | Who writes it |
|---|---|---|
| `TenantResolver` | request → `TenantId` | you, or a built-in (`HeaderTenantResolver`, `PathTenantResolver`, `ExtensionTenantResolver`, `FnTenantResolver`) |
| `TenantSource<T>` | tenant → `T` | you (or `PoolSource` for databases) |
| `Tenanted<T>` | the map holding every tenant's `T` | the framework |

You wire them with two plugins:

```rust
use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};
use std::time::Duration;

AppBuilder::new()
    // 1. how a request names its tenant (the x-tenant-id header)
    .provide(HeaderTenantResolver::default())
    .plugin(Tenancy::resolver::<HeaderTenantResolver>())

    // 2. how a tenant gets its pool
    .provide(PoolDirectory { directory })
    .plugin(PerTenant::<PgPool>::from::<PoolDirectory>()
        .max_active(200)
        .idle_ttl(Duration::from_secs(600)))
```

Add one `PerTenant` plugin per per-tenant resource type. Both plugins declare
their beans as dependencies, so naming a bean you never provided is a compile
error.

## Using it in a controller

`Tenant<T>` and `TenantId` are `#[inject(request)]` fields:

```rust
#[controller(path = "/orders")]
pub struct OrderController {
    #[inject(request)] db: Tenant<PgPool>,   // this request's tenant's pool
    #[inject(request)] tenant: TenantId,     // just the id, provisions nothing
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> JsonResult<Vec<Order>> {
        Ok(Json(sqlx::query_as("select * from orders").fetch_all(&*self.db).await?))
    }
}
```

`Tenant<T>` derefs to `T` and also offers `.tenant_id()`, `.get()`,
`.into_inner()` and `.into_parts()`. Use `Option<Tenant<T>>` or
`Option<TenantId>` for routes that may run without a tenant (this needs
`on-missing: allow`).

If the `Tenancy` plugin is missing, `register_controller` fails to compile with
"type `TenantRouter` was not provided to the AppBuilder".

## Configuration (`tenancy.*`)

```yaml
tenancy:
  enabled: true           # false: the app boots, nothing resolves
  on-missing: reject      # or `allow`
  missing-status: 400     # no tenant in the request
  unknown-status: 404     # the source returned Ok(None)
  unavailable-status: 503 # the source returned Err(..)
  max-active: 500         # soft trim target per map, LRU beyond
  idle-ttl: 15m           # evict after this idle time (0 = never)
  create-timeout: 10s     # per `create` call; exceeding it is a 504 (0 = none)
  negative-ttl: 5s        # how long an unknown tenant is remembered
  max-negative: 1024      # cap on remembered unknown tenants
```

Precedence: `PerTenant` builder > `tenancy.*` file > built-in default.

`on-missing: reject` is the default on purpose: a request without a tenant is
malformed, and failing closed keeps it from reading anyone's data.

### Builder methods

| On | Method | Effect |
|---|---|---|
| `PerTenant` | `.max_active(n)` | Soft cap on live entries (must be at least 1) |
| `PerTenant` | `.idle_ttl(d)` / `.keep_forever()` | Idle eviction |
| `PerTenant` | `.create_timeout(d)`, `.negative_ttl(d)` | Source call timeout, unknown-tenant cache |
| `PerTenant` | `.eager([ids])` | Preload tenants at serve time |
| `PerTenant` | `.fallback_to_default()` | Serve the app-scoped `T` bean for unknown tenants |
| `Tenancy` | `.require_tenant()` / `.allow_missing_tenant()` | Override `on-missing` |
| `Tenancy` | `.statuses(TenantStatuses { .. })` | Override the three statuses |

## Per-tenant SQLx transactions

With `tenant-sqlx`, `PoolSource` turns a tenant into a pool, and `TenantTx` is a
`#[managed]` transaction on the requesting tenant's database:

```rust
use r2e::r2e_data_sqlx::{PoolSource, TenantTx};
use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};
use sqlx::{Pool, Postgres};

AppBuilder::new()
    .provide(HeaderTenantResolver::default())
    .plugin(Tenancy::resolver::<HeaderTenantResolver>())
    // tenant -> DSN. Ok(None) = unknown tenant (404), Err = directory down (503).
    .provide(PoolSource::<Postgres>::new(move |tenant| {
        let master = master.clone();
        async move {
            Ok(sqlx::query_scalar("SELECT dsn FROM tenants WHERE slug = $1")
                .bind(tenant.as_str())
                .fetch_optional(&master)
                .await?)
        }
    }).max_connections(4))                    // per tenant
    .plugin(PerTenant::<Pool<Postgres>>::from::<PoolSource<Postgres>>().max_active(200))
```

```rust
#[post("/orders")]
async fn create(&self, #[managed] tx: &mut TenantTx<'_, Postgres>) -> Result<StatusCode, HttpError> {
    sqlx::query("INSERT INTO orders(name) VALUES ($1)")
        .bind("Ada")
        .execute(tx.connection())
        .await?;
    Ok(StatusCode::CREATED) // commits on that tenant's database
}
```

The Diesel backend (`tenant-diesel`) has the same shape.

Steady-state connections are roughly `max_connections × live tenants`.
`max_active` is a soft trim target, not admission control, so a burst of cold
tenants can briefly exceed it.

## Testing

```rust
#[r2e::test(app = MyApp)]
async fn each_tenant_reads_its_own_database(app: TestApp) {
    app.get("/notes").as_tenant("acme").send().await.assert_ok();
    app.get("/notes").send().await.assert_bad_request();               // no tenant
    app.get("/notes").as_tenant("ghost").send().await.assert_not_found();
}
```

`.as_tenant(id)` sets the `x-tenant-id` header. `.as_tenant_user(sub, tenant, roles)`
also mints a JWT carrying a `tenant` claim. Both work on `TestApp` requests and
on `TestSession`.

Examples: `examples/example-multi-tenant-db` (database per tenant) and
`examples/example-multi-tenant` (row-level, with a custom guard).

## See also

- Full guide: [docs/features/24-tenancy.md](https://github.com/plawn/r2e/blob/master/docs/features/24-tenancy.md)
- [Transactions](../data-access/transactions.md)

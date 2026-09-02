---
topic: tenancy-datasources
features: tenant-sqlx | tenant-diesel
tokens: ~3300
requires: tenancy, managed-resources
---

## Multi-Tenancy — per-tenant SQLx / Diesel pools

### TL;DR

- Requires feature `tenant-sqlx` / `tenant-diesel` (+ a driver): database-per-tenant, with the same commit/rollback lifecycle as `Tx`, on the requesting tenant's pool.
- Use the ready-made `PoolSource<DB>` (tenant → DSN → pool) instead of hand-writing a `TenantSource` for pools; `PoolSource::sync(|tenant| ..)` when the lookup needs no `.await`.
- The bean is `TenantPools<DB>` (alias for `Tenanted<Pool<DB>>`), installed with `PerTenant::<Pool<DB>>::from::<PoolSource<DB>>()`; inject it as a plain bean for ops endpoints.
- The transaction is `#[managed] tx: &mut TenantTx<'_, DB>` (SQLx) or `TenantTx<Conn>` (Diesel, no lifetime param); use `tx.connection()` / `tx.run(|c| ..)`, and `tx.tenant()` to see which tenant it ran for.
- The route declares nothing else: `TenantPool` lists `TenantRouter` + `TenantPools` in its `TxSource::Deps`, so a missing `Tenancy` / `PerTenant` plugin is a compile error at `register_controller`.
- An optional `#[inject(request)] tenant: TenantId` / `Tenant<Pool<DB>>` field makes the transaction reuse the tenant it already resolved.
- `max_connections` on the source is **per tenant** — steady-state connections are roughly `max_connections × live tenants`, and `max_active` is soft, so leave burst headroom or add admission control.
- A changed DSN is `pools.invalidate(&tenant)`, not a `DbPool`-style rotation; `dispose` closes an evicted tenant's SQLx pool, while r2d2 pools have no close and are released by `Drop`.
- Per-tenant **migrations are out of scope**: run them from your provisioning path, never the request path.
- Diesel differences: keyed by the connection type, `with_factory(|dsn| ..)` replaces `with_options`, pools build inside `spawn_blocking`. In tests: `.as_tenant("acme")` or `.as_tenant_user("alice", "acme", &["admin"])`.

This is the per-tenant bean machinery of llm/tenancy.md applied to database
pools: the same resolver and `PerTenant` wiring, with the framework supplying
the `TenantSource`.

Requires feature: `tenant-sqlx` (+ a driver: `sqlx-sqlite` / `sqlx-postgres` /
`sqlx-mysql`). Database-per-tenant, with the same commit/rollback lifecycle as
`Tx`, on the requesting tenant's pool.

| Type | What it is |
|---|---|
| `TenantPools<DB>` | the bean — alias for `Tenanted<Pool<DB>>` |
| `PoolSource<DB>` | ready-made `TenantSource<Pool<DB>>`: tenant → DSN → pool |
| `TenantTx<'_, DB>` | the `#[managed]` transaction on that tenant's pool |
| `TenantPool<DB>` | its `TxSource` marker (rarely named directly) |

```rust
use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};
use r2e::r2e_data_sqlx::{PoolSource, TenantTx};
use sqlx::{Pool, Postgres};

# async fn wiring(master: Pool<Postgres>) -> impl Sized {
AppBuilder::new()
    .provide(HeaderTenantResolver::default())              // x-tenant-id
    .plugin(Tenancy::resolver::<HeaderTenantResolver>())
    // tenant → DSN. Ok(None) = unknown tenant (404), Err = directory down (503).
    .provide(PoolSource::<Postgres>::new(move |tenant| {
        let master = master.clone();
        async move {
            Ok(sqlx::query_scalar("SELECT dsn FROM tenants WHERE slug = $1")
                .bind(tenant.as_str())
                .fetch_optional(&master)
                .await?)
        }
    }).max_connections(4))                                 // per tenant!
    .plugin(PerTenant::<Pool<Postgres>>::from::<PoolSource<Postgres>>()
        .max_active(200))                                  // soft live-pool trim target
# }

#[controller(path = "/orders")]
pub struct OrderController;                                // no fields at all

#[routes]
impl OrderController {
    #[post("/")]
    async fn create(&self, #[managed] tx: &mut TenantTx<'_, Postgres>)
        -> Result<StatusCode, HttpError> {
        sqlx::query("INSERT INTO orders(name) VALUES ($1)")
            .bind("Ada").execute(tx.connection()).await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        tx.tenant();                            // which tenant this ran for
        Ok(StatusCode::CREATED)                 // commits on that tenant's database
    }
}
# fn main() {}
```

The route declares **nothing** beyond the `#[managed]` parameter: `TenantPool`
lists `TenantRouter` + `TenantPools<DB>` in its `TxSource::Deps`, so a missing
`Tenancy` / `PerTenant` plugin is a compile error at `register_controller`. A
`#[inject(request)] tenant: TenantId` / `Tenant<Pool<DB>>` field is optional —
when present, the transaction reuses the tenant it already resolved.
`PoolSource::sync(|tenant| ...)` for a lookup that needs no `.await`;
`.with_options(PgPoolOptions::new()...)` for full pool control. `dispose` closes
an evicted tenant's pool. A changed DSN is `pools.invalidate(&tenant)` (no
`DbPool`-style rotation involved). Per-tenant **migrations are out of scope** —
run them from your provisioning path, not the request path. In tests:
`app.post("/orders").as_tenant("acme")` (header) or
`.as_tenant_user("alice", "acme", &["admin"])` (header + `tenant` JWT claim).
Steady-state connections are roughly `max_connections × live tenants`, but
`max_active` is soft: leave burst headroom or add admission control.

### Per-tenant Diesel pools & transactions

Requires feature: `tenant-diesel` (+ a driver: `diesel-sqlite` /
`diesel-postgres` / `diesel-mysql`). The same shapes as the SQLx version, over
r2d2 pools and keyed by the **connection type** (`Conn`), not a `DB` marker.

| Type | What it is |
|---|---|
| `TenantPools<Conn>` | the bean — alias for `Tenanted<Pool<ConnectionManager<Conn>>>` |
| `PoolSource<Conn>` | ready-made `TenantSource<Pool<ConnectionManager<Conn>>>`: tenant → DSN → pool |
| `TenantTx<Conn>` | the `#[managed]` transaction on that tenant's pool (no lifetime param) |
| `TenantPool<Conn>` | its `TxSource` marker (rarely named directly) |

```rust
use r2e::tenant::PerTenant;
use r2e::r2e_data_diesel::{PoolSource, TenantTx};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::prelude::*;                                    // PgConnection, RunQueryDsl, …

// Resolver wiring is identical to the SQLx block above.
# async fn wiring(master: MasterDb) -> impl Sized {
AppBuilder::new()
    // tenant → DSN. Ok(None) = unknown tenant (404), Err = directory down (503).
    .provide(PoolSource::<PgConnection>::new(move |tenant| {
        let master = master.clone();
        async move { Ok(master.dsn_for(tenant.as_str()).await?) }
    }).max_connections(4))                                 // per tenant!
    .plugin(PerTenant::<Pool<ConnectionManager<PgConnection>>>::from::<PoolSource<PgConnection>>()
        .max_active(200))                                  // soft live-pool trim target
# }

#[controller(path = "/orders")]
pub struct OrderController;                                // no fields at all

#[routes]
impl OrderController {
    #[post("/")]
    async fn create(&self, #[managed] tx: &mut TenantTx<PgConnection>)
        -> Result<StatusCode, HttpError> {
        tx.run(|c| diesel::insert_into(orders::table).values(&new).execute(c)).await?;
        tx.tenant();                            // which tenant this ran for
        Ok(StatusCode::CREATED)                 // commits on that tenant's database
    }
}
# fn main() {}
```

Same compile-time guarantee (`TenantPool` lists `TenantRouter` +
`TenantPools<Conn>` in its `TxSource::Deps`), same memoized tenant, same
negative caching, same migrations deferral. Differences from the SQLx source:
`PoolSource::with_factory(|dsn| ...)` replaces `with_options` (r2d2's `Builder`
is not clonable), pools are built inside `spawn_blocking`, and there is **no**
`dispose` — r2d2 pools have no close, so an evicted tenant's pool is released by
`Drop` once its last handle and connection go away.

### End-to-end (database-per-tenant)

The whole shape in one app: a resolver, the framework's `PoolSource`, a cascaded
per-tenant client, a fallback resource, and the maps as ops beans. Runnable
version: `examples/example-multi-tenant-db` (feature `tenant-sqlx`).

```rust
use r2e::prelude::*;
use r2e::r2e_data_sqlx::{PoolSource, TenantPools, TenantTx};
use r2e::tenant::{BoxError, BoxFuture, PerTenant, SyncTenantResolver, Tenancy,
                  Tenant, TenantContext, TenantId, TenantSource};
use sqlx::{Pool, Sqlite};

// ── SPI #1: request → tenant ───────────────────────────────────────────────
#[derive(Clone, Default)]
pub struct HeaderResolver;

impl SyncTenantResolver for HeaderResolver {
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        match req.header("x-tenant-id") {
            None => Ok(None),                       // → `tenancy.on-missing` decides
            Some(raw) => TenantId::parse(raw).map(Some)
                .map_err(|e| HttpError::bad_request(format!("invalid tenant: {e}"))),
        }
    }
}

// ── SPI #2: a per-tenant client built on the SAME tenant's pool (cascade) ──
#[derive(Clone)]
pub struct ApiClient { tenant: TenantId, token: String, db: Pool<Sqlite> }

#[derive(Clone, Default)]
pub struct ApiClients;

impl TenantSource<ApiClient> for ApiClients {
    fn create<'a>(&'a self, tenant: &'a TenantId, ctx: &'a TenantContext<'a>)
        -> BoxFuture<'a, Result<Option<ApiClient>, BoxError>> {
        Box::pin(async move {
            let db = ctx.get::<Pool<Sqlite>>().await?;          // this tenant's pool
            let master = ctx.bean::<MasterDb>().ok_or("MasterDb not provided")?;
            let Some(token) = master.api_token(tenant.as_str()).await? else {
                return Ok(None);                                // unknown → 404
            };
            Ok(Some(ApiClient { tenant: tenant.clone(), token, db }))
        })
    }
}

// ── Wiring ─────────────────────────────────────────────────────────────────
pub struct MultiTenantDbApp;

impl App for MultiTenantDbApp {
    type Env = AppEnv;

    async fn setup() -> Result<AppEnv, BootError> {
        Ok(AppEnv { master: provision().await? })
    }

    async fn build(b: AppBuilder, env: AppEnv) -> Result<impl BootableApp, BootError> {
        let master = env.master.clone();
        let pool_source = PoolSource::<Sqlite>::new(move |tenant| {
            let master = master.clone();
            async move { Ok(master.dsn(tenant.as_str()).await?) }
        }).max_connections(2);                        // per tenant!

        Ok(b.load_config::<()>()
            .provide(env.master)
            .provide(HeaderResolver)
            .plugin(Tenancy::resolver::<HeaderResolver>())
            .provide(pool_source)
            .plugin(PerTenant::<Pool<Sqlite>>::from::<PoolSource<Sqlite>>()
                .max_active(16)                       // soft live-pool trim target
                .idle_ttl(Duration::from_secs(300)))
            .provide(ApiClients)
            .plugin(PerTenant::<ApiClient>::from::<ApiClients>().max_active(16))
            .provide(Branding::shared())              // the fallback value
            .provide(Brandings)
            .plugin(PerTenant::<Branding>::from::<Brandings>().fallback_to_default())
            .plugin(Health)
            .try_build_state().await?
            .register_controllers::<(NotesController, ClientController, AdminController)>())
    }
}

// ── Using it: no controller names a tenant ─────────────────────────────────
#[controller(path = "/notes")]
pub struct NotesController;                           // no fields at all

#[routes]
impl NotesController {
    #[get("/")]
    async fn list(&self, #[managed] tx: &mut TenantTx<'_, Sqlite>)
        -> Result<Json<Vec<String>>, HttpError> {
        Ok(Json(sqlx::query_scalar("SELECT body FROM notes")
            .fetch_all(tx.connection()).await
            .map_err(|e| HttpError::internal(e.to_string()))?))
    }
}

#[controller(path = "/client")]
pub struct ClientController {
    #[inject(request)] client: Tenant<ApiClient>,     // cascaded, per request
}

#[routes]
impl ClientController {
    #[get("/")]
    async fn token(&self) -> Json<String> { Json(self.client.token.clone()) }
}

#[controller(path = "/admin")]
pub struct AdminController {
    #[inject] pools: TenantPools<Sqlite>,             // the map is a plain bean
}

#[routes]
impl AdminController {
    #[get("/pools")]
    async fn active(&self) -> Json<Vec<TenantId>> { Json(self.pools.active()) }
}
# fn main() {}
```

```bash
curl -H 'x-tenant-id: acme'   localhost:3000/notes   # acme's own database
curl -H 'x-tenant-id: globex' localhost:3000/notes   # globex's own database
curl localhost:3000/notes                            # 400 — no tenant
curl -H 'x-tenant-id: ghost'  localhost:3000/notes   # 404 — unknown tenant
curl localhost:3000/admin/pools                      # active / stats / metrics
```

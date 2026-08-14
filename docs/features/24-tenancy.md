# Feature 24 — Multi-Tenancy (per-tenant beans)

## TL;DR

One process, many tenants, each with its **own** database pool, API client or feature set — resolved from the request, created on first use, reclaimed when idle. Three moving parts: a `TenantResolver` (request → `TenantId`, you write it or use a built-in), a `TenantSource<T>` (tenant → `T`, you write it), and `Tenanted<T>` (the map, the framework's). Wiring is two plugins; *using* it is one request-scoped field — `#[inject(request)] db: Tenant<PgPool>` — and the handler never names a tenant. Forgetting a plugin is a **compile error** at `register_controller`, not a 500 on the first request from the first tenant in production. Failures have one status each (400 missing · 404 unknown · 503 unavailable · 504 timeout · 500 wiring bug), the request-driven three configurable. Requires feature `tenant`; `tenant-sqlx` / `tenant-diesel` add ready-made per-tenant pools and `#[managed]` transactions.

## Objective

Database-per-tenant (and client-per-tenant, config-per-tenant, …) without a `HashMap<String, Pool>` in shared state and a `tenant` argument threaded through every function. The tenant becomes an ambient, compile-checked property of the request — the same way the identity is.

This is **not** the row-level model (one database, a `tenant_id` column, guards enforcing isolation). That model needs no framework support; see `examples/example-multi-tenant`. This feature is for when each tenant's resources are physically separate.

## Feature Flag

```toml
r2e = { features = ["tenant"] }                       # the layer itself
r2e = { features = ["tenant-sqlx", "sqlx-postgres"] } # + per-tenant SQLx pools
r2e = { features = ["tenant-diesel", "diesel-postgres"] }
```

`tenant` is included in `full`; `tenant-sqlx` / `tenant-diesel` are **not** (they pull a database backend). Crate: `r2e-tenant`, re-exported as `r2e::tenant`.

## Core Concepts

### The three pieces

| Piece | What it is | Who writes it |
|---|---|---|
| `TenantResolver` | request → `TenantId` | you, or a built-in (`HeaderTenantResolver`, …) |
| `TenantSource<T>` | tenant → `T` | you (or `PoolSource` for databases) |
| `Tenanted<T>` | the map holding every tenant's `T` | the framework |

Wiring is two pre-state plugins:

```rust
use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy};

AppBuilder::new()
    // 1. how a request names its tenant
    .provide(HeaderTenantResolver::default())            // x-tenant-id
    .plugin(Tenancy::resolver::<HeaderTenantResolver>()) // provides TenantRouter

    // 2. how a tenant gets its pool
    .provide(PoolDirectory { directory })
    .plugin(PerTenant::<PgPool>::from::<PoolDirectory>() // provides Tenanted<PgPool>
        .max_active(200)
        .idle_ttl(Duration::from_secs(600)))
```

`PerTenant` is repeated once per per-tenant resource type. Both plugins declare their source as a `Deps`, so naming a bean you never provided fails to compile.

### `TenantId`

Shape: `[a-z0-9][a-z0-9._-]{0,62}` — lowercase ASCII alphanumerics, dots, dashes and underscores, first character alphanumeric, at most `MAX_TENANT_ID_LEN` (63) bytes. 63 is the shortest of the practical downstream limits (DNS label, Postgres identifier truncation, S3 bucket-name segment).

It is **parsed, never deserialized** — there is deliberately no `Deserialize` impl, so a tenant id cannot arrive inside a request body and skip validation. A tenant id picks a database, a schema or a bucket prefix, so `../etc/passwd`, `ACME` and the empty string are rejected at the edge by one function.

```rust
TenantId::parse("acme-eu")?;                 // Result<_, InvalidTenantId>
TenantId::parse_owned(string)?;              // no re-allocation
TenantId::from_static_unchecked("acme");     // fixtures / trusted stores only
id.as_str();
```

`Arc<str>` inside: cheap to clone, usable as a map key (`Eq + Hash + Ord`), `Display`, `FromStr`, `TryFrom<&str>`, `TryFrom<String>`, `Serialize` (one-way — it can be *returned*, not *received*).

### Resolvers (SPI #1)

A resolver is a bean. It runs once per request, before any per-tenant resource is touched, and its answer is memoized in `parts.extensions` — so extractors, guards and `#[managed]` resources of one request resolve **once**.

Its three answers:

| Return | Meaning |
|---|---|
| `Ok(Some(id))` | this request's tenant |
| `Ok(None)` | "no tenant here" — what happens next is `tenancy.on-missing`'s call, not the resolver's |
| `Err(HttpError)` | a *malformed* tenant (present but not a valid id). The resolver owns that status |

Built-ins:

```rust
HeaderTenantResolver::default()                       // x-tenant-id
HeaderTenantResolver::new("x-org")
PathTenantResolver::new("org")                        // /{org}/... path param
ExtensionTenantResolver::<MyClaims, _>::new(|c: &MyClaims| TenantId::parse(&c.tenant).ok())
FnTenantResolver::new(|req: &RequestHead<'_>| { ... })
```

Writing one is a few lines of `SyncTenantResolver` (blanket-bridged to `TenantResolver`; implement the async `TenantResolver` directly only when resolution needs `.await`):

```rust
use r2e::tenant::{SyncTenantResolver, TenantId};

#[derive(Clone)]
struct SubdomainResolver;

impl SyncTenantResolver for SubdomainResolver {
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        let Some(host) = req.host() else { return Ok(None) };
        let Some((label, _)) = host.split_once('.') else { return Ok(None) };
        TenantId::parse(label)
            .map(Some)
            .map_err(|e| HttpError::bad_request(format!("invalid tenant subdomain: {e}")))
    }
}
```

**Tenants from a JWT.** There is deliberately no JWT resolver: it would duplicate the security layer's validation and put a second JWT parse on every request. Instead the identity extractor (or a middleware) parks what it already parsed, and `ExtensionTenantResolver` reads it back:

```rust
// in the identity extractor / a middleware:
parts.extensions.insert(TenantClaim(claims.tenant.clone()));

// wiring:
.provide(ExtensionTenantResolver::<TenantClaim, _>::new(|c: &TenantClaim| {
    TenantId::parse(&c.0).ok()
}))
.plugin(Tenancy::resolver::<ExtensionTenantResolver<TenantClaim, _>>())
```

Extraction order makes this work: the request-data extractor (which holds the identity) runs before the per-tenant extractors on the same route.

### Sources (SPI #2)

```rust
use r2e::tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantSource};

impl TenantSource<PgPool> for PoolDirectory {
    fn create<'a>(&'a self, tenant: &'a TenantId, ctx: &'a TenantContext<'a>)
        -> BoxFuture<'a, Result<Option<PgPool>, BoxError>> {
        Box::pin(async move {
            let Some(url) = self.directory.dsn(tenant).await? else {
                return Ok(None);          // unknown tenant → 404, negatively cached
            };
            Ok(Some(PgPool::connect(&url).await?))
        })
    }

    // optional; default does nothing
    fn dispose<'a>(&'a self, tenant: &'a TenantId, value: PgPool) -> BoxFuture<'a, ()> {
        Box::pin(async move { value.close().await })
    }
}
```

Again three answers, and they map straight onto statuses: `Ok(Some(v))` provisioned, `Ok(None)` unknown tenant (404), `Err` the resource could not be built (503, **not** cached, retried on the next request).

`dispose` is called when a resource is evicted (idle, LRU, `evict()`, or shutdown drain) — the hook where a pool is actually closed rather than left to `Drop`.

### The cascade

`create` receives a `TenantContext`, which resolves other resources **for the same tenant**:

| Call | Resolves |
|---|---|
| `ctx.get::<U>().await?` | `U` for this tenant, through `U`'s own source (lazy, single-flighted, cycle-detected) |
| `ctx.bean::<U>()` | a plain app-scoped bean out of the graph |
| `ctx.tenant()` | the tenant being built for |
| `ctx.chain()` | the resolution chain so far, for diagnostics |

So a per-tenant API client can be built on that tenant's per-tenant pool, and neither source knows how the other is wired. A loop (A needs B needs A) is `TenantError::Cycle`, which names the chain.

### Using it

`Tenant<T>` and `TenantId` are **`#[inject(request)]` controller fields** (a field attribute, not a handler-parameter one):

```rust
#[controller(path = "/orders")]
struct OrderController {
    #[inject(request)] db: Tenant<PgPool>,   // this request's tenant's pool
    #[inject(request)] tenant: TenantId,     // just the id — provisions nothing
    // Option<Tenant<T>> / Option<TenantId> = "no tenant" (never "bad tenant")
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> JsonResult<Vec<Order>> {
        Ok(Json(sqlx::query_as("select * from orders").fetch_all(&*self.db).await?))
    }
}
```

`Tenant<T>` derefs to `T` and exposes `.tenant_id()`, `.get()`, `.into_inner()`, `.into_parts()`.

Neither extractor implements axum's `FromRequestParts` (bridge-overlap invariant) — they are `#[inject(request)]` fields only, extracted through R2E's `FromRequestPartsVia`.

### Forgetting the wiring is a compile error

Both extractors read their beans out of the HList state, so a missing plugin fails at `register_controller` / `register_controllers`:

```text
error[E0277]: type `TenantRouter` was not provided to the AppBuilder
   |
31 |             .register_controller::<OrderController>()
   |                                    ^^^^^^^^^^^^^^^ missing `.provide::<TenantRouter>()` or `.register::<TenantRouter>()`
```

Covered by `r2e-compile-tests/cases/tenancy/fail/`.

## Configuration

Everything under `tenancy.*` (`TenancyConfig`, `CONFIG_PREFIX = "tenancy"`):

```yaml
tenancy:
  enabled: true          # false → the app boots and compiles; nothing resolves
  on-missing: reject     # or `allow` (Option<Tenant<T>> then yields None)
  missing-status: 400    # no tenant in the request
  unknown-status: 404    # the source said Ok(None)
  unavailable-status: 503 # the source said Err(..)
  max-active: 500        # live resources per map, LRU beyond
  idle-ttl: 15m          # evict + dispose after this idleness (0 = never)
  create-timeout: 10s    # per `create` call; blowing it is a 504 (0 = none)
  negative-ttl: 5s       # how long an unknown tenant is remembered (0 = none)
  max-negative: 1024     # cap on remembered unknown tenants
```

Precedence:

```text
PerTenant builder  >  tenancy.* (file)  >  built-in default
```

Builder methods: `.max_active(n)`, `.idle_ttl(d)`, `.keep_forever()`, `.create_timeout(d)`, `.negative_ttl(d)`, `.eager([ids])` (preload at serve), `.fallback_to_default()`. On the `Tenancy` plugin: `.require_tenant()`, `.allow_missing_tenant()`, `.statuses(TenantStatuses { .. })`.

**`on-missing: reject` is the default deliberately** — it applies to `Option<Tenant<T>>` / `Option<TenantId>` too. In a multi-tenant deployment a request without a tenant is malformed, and failing closed keeps it from serving anyone's data.

### Fallback

`.fallback_to_default()` makes a tenant the source returns `Ok(None)` for get the **app-scoped `T` bean** rather than a 404 — the shape for a resource not every tenant has (custom branding, a feature-flag override). The fallback value is never disposed and never cached per tenant, and the call adds `T` to the plugin's `Deps`, so the `.provide(T)` above it is compile-checked rather than hoped for.

Fallback is per resource, not per app: with a per-tenant pool strict and per-tenant branding falling back, an unknown tenant is a 404 on `/notes` and a 200 on `/branding`.

## Failure Mapping

| `TenantError` | Status | Configurable |
|---|---|---|
| `Unresolved` | 400 | `missing-status` |
| `Unknown(id)` | 404 | `unknown-status` |
| `Unavailable { .. }` | 503 | `unavailable-status` |
| `Timeout(id)` | 504 | no |
| `Cycle(chain)` | 500 | no (a bug) |
| `NoResolver` | 500 | no (a bug) |
| `NoSource(ty)` | 500 | no (a bug) |

The three request-driven statuses are configurable because their right value depends on the deployment — a gateway that maps the tenant itself may prefer 401/403 for a missing tenant. The three bug statuses are not: a 500 is the correct answer. `TenantError` exposes `.tenant()`, `.is_bug()`, `.into_http_error(statuses)`, and `TenantError::unavailable(id, source)`.

## Operating the maps

`Tenanted<T>` is an ordinary app-scoped bean — `#[inject] pools: Tenanted<PgPool>` from a controller that has nothing to do with a tenant request:

| Method | Does |
|---|---|
| `get(&id).await` | create-or-cached, single-flighted |
| `peek(&id)` | cached only, no creation |
| `active()` | `Vec<TenantId>` of live resources |
| `stats()` | `Vec<TenantStats>` — `tenant`, `ready`, `idle` |
| `metrics()` | `TenantedMetrics` — `active`, `negative`, `hits`, `created`, `create_failures`, `timeouts`, `unknown`, `fallbacks`, `disposed`, `evicted_idle`, `evicted_lru` |
| `evict(&id).await` | drop **and dispose** |
| `invalidate(&id)` | drop **now**, dispose in the background — the "its DSN changed" shape: the caller doesn't wait, in-flight requests finish on the old resource, the next one rebuilds |
| `preload(ids).await` | warm resources up front; returns the per-tenant failures |
| `sweep().await` | one idle/LRU/negative pass, returning a `SweepReport` |
| `drain().await` | dispose everything (wired to shutdown automatically) |

Lifecycle guarantees: concurrent first requests for a tenant share **one** creation; failures are **not** cached; unknown tenants are negative-cached for `negative-ttl` and cleared as soon as the tenant becomes known; creation is bounded by `create-timeout`; idle resources are evicted and disposed by a background sweep; shutdown drains them.

## Per-tenant SQLx pools & transactions

Feature `tenant-sqlx` + a driver (`sqlx-sqlite` / `sqlx-postgres` / `sqlx-mysql`). Database-per-tenant, with the same commit/rollback lifecycle as the single-tenant `Tx`, on the requesting tenant's pool.

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

AppBuilder::new()
    .provide(HeaderTenantResolver::default())
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
        .max_active(200))                                  // caps total connections
```

```rust
#[post("/orders")]
async fn create(&self, #[managed] tx: &mut TenantTx<'_, Postgres>)
    -> Result<StatusCode, HttpError> {
    sqlx::query("INSERT INTO orders(name) VALUES ($1)")
        .bind("Ada").execute(tx.connection()).await?;
    tx.tenant();                              // which tenant this ran for
    Ok(StatusCode::CREATED)                   // commits on that tenant's database
}
```

The route declares **nothing** beyond the `#[managed]` parameter: `TenantPool` lists `TenantRouter` + `TenantPools<DB>` in its `TxSource::Deps`, so a missing plugin is a compile error. A `#[inject(request)] tenant: TenantId` / `Tenant<Pool<DB>>` field is optional — when present, the transaction reuses the tenant it already resolved.

Also: `PoolSource::sync(|tenant| ...)` for a lookup needing no `.await`, `.with_options(PgPoolOptions::new()...)` for full pool control, `.max_connections(n)` for the common case. `dispose` closes an evicted tenant's pool.

**Total connections = `max_connections` × live tenants**, which `max_active` caps. Size both together.

## Per-tenant Diesel pools & transactions

Feature `tenant-diesel` + a driver (`diesel-sqlite` / `diesel-postgres` / `diesel-mysql`). The same shapes over r2d2 pools, keyed by the **connection type** (`Conn`), not a `DB` marker.

| Type | What it is |
|---|---|
| `TenantPools<Conn>` | alias for `Tenanted<Pool<ConnectionManager<Conn>>>` |
| `PoolSource<Conn>` | ready-made `TenantSource<Pool<ConnectionManager<Conn>>>` |
| `TenantTx<Conn>` | the `#[managed]` transaction (no lifetime param) |
| `TenantPool<Conn>` | its `TxSource` marker |

```rust
use r2e::r2e_data_diesel::{PoolSource, TenantTx};
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::PgConnection;

.provide(PoolSource::<PgConnection>::new(move |tenant| {
    let master = master.clone();
    async move { Ok(master.dsn_for(tenant.as_str()).await?) }
}).max_connections(4))
.plugin(PerTenant::<Pool<ConnectionManager<PgConnection>>>::from::<PoolSource<PgConnection>>()
    .max_active(200))

#[post("/orders")]
async fn create(&self, #[managed] tx: &mut TenantTx<PgConnection>)
    -> Result<StatusCode, HttpError> {
    tx.run(|c| diesel::insert_into(orders::table).values(&new).execute(c)).await?;
    tx.tenant();
    Ok(StatusCode::CREATED)
}
```

Differences from the SQLx source: `PoolSource::with_factory(|dsn| ...)` replaces `with_options` (r2d2's `Builder` is not clonable), pools are built inside `spawn_blocking`, and there is **no** `dispose` — r2d2 pools have no close, so an evicted tenant's pool is released by `Drop` once its last handle and connection go away.

## Testing

```rust
#[r2e::test(app = MyApp)]
async fn each_tenant_reads_its_own_database(app: TestApp) {
    app.get("/notes").as_tenant("acme").send().await
        .assert_ok()
        .assert_json_path("tenant", "acme");

    app.get("/notes").send().await.assert_bad_request();          // no tenant
    app.get("/notes").as_tenant("ghost").send().await.assert_not_found();
}
```

`.as_tenant(id)` sets the `x-tenant-id` header. `.as_tenant_user(sub, tenant, roles)` mints a JWT carrying a `tenant` claim **and** sets the header, for apps whose resolver reads the identity. Both are available on `TestApp` requests and on `TestSession`.

## Out of scope

**Per-tenant migrations.** Run them from your provisioning path (when a tenant signs up), never on the request path — a migration on a request means the first request after a deploy pays for it, under whatever concurrency arrives. The framework will not run them for you.

**A tenant directory.** R2E does not know what a tenant is; it routes to whatever your source says. The directory is a Postgres table, a control-plane API, or a config service — yours.

## Examples

| Example | Model |
|---|---|
| [`example-multi-tenant-db`](../../examples/example-multi-tenant-db/README.md) | **database-per-tenant** — this feature: resolver, `PoolSource`, `TenantTx`, cascade, fallback, admin/ops routes |
| [`example-multi-tenant`](../../examples/example-multi-tenant/README.md) | **row-level** — one database, a tenant column, a custom guard. No tenancy layer involved |

## Reference

- `r2e-tenant` module docs: `resolver` (writing one), `source` (the cascade), `map` (lifecycle), `error` (status mapping), `config` (every knob).
- `docs/claude/subsystems.md` § r2e-tenant — internals.

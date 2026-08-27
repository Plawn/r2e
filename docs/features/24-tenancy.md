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

Wiring is two plugins:

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
TenantId::from_static("acme");               // validates; panics if invalid
id.as_str();
```

`TenantId::from_static` is the convenience constructor for fixtures and trusted
static stores, not a validation bypass: it applies the same grammar and panics
when the literal is invalid.

`Arc<str>` inside: cheap to clone, usable as a map key (`Eq + Hash + Ord`), `Display`, `FromStr`, `TryFrom<&str>`, `TryFrom<String>`, `Serialize` (one-way — it can be *returned*, not *received*).

### Resolvers (SPI #1)

A resolver is a bean. The `Tenancy` plugin installs a router layer that parks a
private, `Arc`-backed resolve-once cell in `parts.extensions` before routing.
Guards, extractors and every `#[managed]` acquisition share that cell, including
routes with no tenancy extractor, so whichever component asks first runs the
resolver and every later component sees the same raw `Option<TenantId>`. The
cell stores the resolver answer before the missing-tenant policy is applied;
`None` is memoized, while resolver errors are not.

A bare `TenantId` inserted into extensions by unrelated middleware is not an
authoritative memo and cannot bypass the configured resolver.
`TenantRouter::memoized(&head)` is a read-only peek at the private cell. A
hand-wired router that provides `TenantRouter` without the `Tenancy` plugin must
call `TenantRouter::install_memo(&mut extensions)` in its own pre-routing layer
to get the whole-request guarantee.

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
ExtensionTenantResolver::<MyClaims, _, Strict>::try_new(|c: &MyClaims| {
    TenantId::parse(&c.tenant)
        .map(Some)
        .map_err(|e| HttpError::bad_request(format!("invalid tenant claim: {e}")))
})
FnTenantResolver::new(|req: &RequestHead<'_>| { ... })
```

`ExtensionTenantResolver` defaults to the `Lenient` mode: `new` projects
`Fn(&T) -> Option<TenantId>`, so an absent extension and a malformed value both
become "no tenant". The `Strict` mode selected by `try_new` projects
`Fn(&T) -> Result<Option<TenantId>, HttpError>`; an absent extension is still
`Ok(None)`, while a present-but-malformed claim can return a 400. `Lenient` and
`Strict` are re-exported from `r2e::tenant`.

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

Extraction order makes this work with either a struct-level identity or a
parameter-level `#[inject(identity)]`: generated handlers extract all
`FromRequestParts` parameters, including parameter identity, before taking the
request-head snapshot used by guards and `#[managed]` resources.

One combination remains unsupported by construction: a controller-field
`#[inject(request)] Tenant<T>` / `TenantId` whose extension resolver depends on
a parameter-level identity. Controller request fields must be extracted before
handler parameters, so the claim does not exist yet; resolution fails closed as
a missing tenant (or yields `None` under `on-missing: allow`), never as the wrong
tenant. Put the identity on the controller struct and use `#[anonymous]` for
public routes when the tenant comes from that identity.

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

`dispose` is called at most once per cached value when a resource is evicted
(idle, LRU, `evict()`, or shutdown drain) — the hook where a pool is actually
closed rather than left to `Drop`. At most once, not exactly once: the one-shot
gate is taken before the call, so a `dispose` that panics or is cancelled
mid-await is not retried.

### The cascade

`create` receives a `TenantContext`, which resolves other resources **for the same tenant**:

| Call | Resolves |
|---|---|
| `ctx.get::<U>().await?` | `U` for this tenant, through `U`'s own source (lazy, single-flighted, cycle-detected) |
| `ctx.bean::<U>()` | a plain app-scoped bean out of the graph |
| `ctx.tenant()` | the tenant being built for |
| `ctx.chain()` | the resolution chain so far, for diagnostics |

So a per-tenant API client can be built on that tenant's per-tenant pool, and
neither source knows how the other is wired. Cycle detection is per resolution
path: a sequential loop (A needs B needs A) is `TenantError::Cycle`, which names
the chain. Two concurrent roots can still form an invisible wait-for cycle
(task 1 creates A and awaits B while task 2 creates B and awaits A); that ends
at `create-timeout` with a 504, or hangs when the timeout is disabled. A real
cycle is a wiring bug that sequential resolution reports during development;
keep `create-timeout` enabled in production.

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
  max-active: 500        # soft trim target per map, LRU beyond
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

`max-active` / `.max_active(n)` must be at least 1. Zero panics during config
load, direct map wiring, or `PerTenant::max_active(0)`; use
`tenancy.enabled: false` to turn tenancy off.

**`on-missing: reject` is the default deliberately** — it applies to `Option<Tenant<T>>` / `Option<TenantId>` too. In a multi-tenant deployment a request without a tenant is malformed, and failing closed keeps it from serving anyone's data.

### Fallback

When the source returns `Ok(None)` for a resolved tenant,
`.fallback_to_default()` serves the **app-scoped `T` bean** rather than a 404 —
the shape for a resource not every tenant has (custom branding, a feature-flag
override). A missing tenant never reaches this fallback; `TenantRouter` rejects
it or an optional extractor yields `None` under `on-missing: allow`. The
fallback value is never disposed and never cached per tenant, and the call adds
`T` to the plugin's `Deps`, so the `.provide(T)` above it is compile-checked
rather than hoped for.

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
| `stats()` | `Vec<TenantStats>` — `tenant`, `ready`, `idle`; serializes directly, with `idle` emitted as whole-millisecond `idle_ms` |
| `metrics()` | serializable `TenantedMetrics` — `active`, `negative`, `hits`, `created`, `create_failures`, `timeouts`, `unknown`, `fallbacks`, `disposed`, `evicted_idle`, `evicted_lru` |
| `evict(&id).await` | remove a **ready** value and await disposal; returns `false` for an in-flight creation |
| `invalidate(&id)` | remove a **ready** value now and spawn disposal in the background; `true` means spawned, not finished, and an in-flight creation returns `false` |
| `preload(ids).await` | warm resources up front; returns the per-tenant failures |
| `sweep().await` | one idle/LRU/negative pass, returning a `SweepReport` |
| `drain().await` | latch the map closed and dispose every ready value (wired to shutdown automatically) |

Lifecycle guarantees and limits:

- Concurrent first requests for a cold tenant share one successful creation.
  Panic or cancellation during `TenantSource::create` removes the empty slot.
  An unknown-tenant cold wave makes exactly one `create` call because the
  negative cache is checked again inside the initializer. Errors are
  deliberately not cached, so an erroring wave can retry once per waiter.
- `evict`, `invalidate`, idle/LRU sweeps and `drain` remove ready slots only;
  an in-flight creation remains mapped. Drain latches the map closed and
  repeatedly removes and disposes ready snapshots until none remain. A creation
  that was already in flight sees the latch when it finishes, removes and
  disposes its own result, then returns the same 503 as subsequent resolutions:
  `the per-tenant resource map is draining (shutdown)`.
- Every **public** removal (`evict`, `invalidate`, the sweeps, `drain`) bumps a
  map-wide epoch *before* it takes the key's shard lock, and each initialization
  stamps the epoch it started at on its **slot** — one reading shared by every
  participant on that cell, rather than a per-caller capture. A creation that was
  already *detached* from the map when a removal
  happened never writes back: it neither reattaches its slot nor records the
  tenant as unknown, so `invalidate` and `evict` stay immediate instead of being
  undone by an older in-flight result. Both write-backs decide and write under
  the same shard guard, which is what orders them against the removal. That value
  is disposed of and still returned to its own caller. The epoch is map-wide, so
  a removal can fence a detached creation for an unrelated tenant — the cost is
  one rebuild on the next request. A creation that is still *mapped* is never
  fenced: removal never touched it, so it overlaps the removal and is
  deliberately left alone. A negative-cache entry is only ever written by the
  attempt that still owns the key — tested and written in one critical section —
  and a resolution consults the ready slot before the negative cache, so an
  "unknown" verdict can never shadow a live resource.
- Each cached slot has a one-shot disposal gate. Normal eviction and drain hand
  its value to `TenantSource::dispose` once; the public source contract is
  "called at most once per cached value." The gate is taken *before* the call,
  so a `dispose` that panics or is cancelled mid-await is not retried — the
  deliberate trade against ever double-disposing. The gate is also what keeps a
  dying value out of the map. The cleanup of a cancelled or panicking
  initializer's *empty* slot deliberately does **not** bump the epoch — that
  would fence off the very waiter that inherits the cell — so two participants
  sharing a cell can classify its value differently when a competing empty slot
  appears under the key and then vanishes. So whoever unmaps or orphans a value
  commits its gate **inline, under the key's shard guard**, in the same critical
  section: either the orphan commits first and the other participant reads the
  committed gate under that same lock and refuses to restore, or the restore
  lands first and the orphan finds its own slot back under the key and keeps it.
  The rule holds with no exceptions — no gate is ever taken outside a shard-lock
  critical section — removals included, which is what makes `evict().await` mean the
  resource is closed when it returns — a participant arriving a moment later
  loses the gate instead of moving the closing onto a detached task. A disposed
  value can never end up mapped, and exactly one caller owes the `dispose` await.
  `invalidate` only spawns that await, and `drain` waits for it: `drain()`
  returns only once every value it is draining is closed. A live value can be
  outside the map and still need closing — a resolve holding a slot that was
  detached under it, or a disposal handed to a detached task — so both are
  counted as in-flight work (the disposal from inside the critical section that
  took the gate) and `drain` waits for that count to hit zero as well as for the
  map to empty. Only *admitted* work is counted: a resolve reads the draining
  latch before it touches the counter, so a request arriving after shutdown
  started is rejected without ever registering, and a sustained flood of
  post-shutdown 503s — the listener is still accepting while the drain hook
  runs — cannot hold `drain` open. That means `drain` also waits for a creation still in flight,
  rather than letting it close itself behind shutdown's back; `create-timeout`
  bounds that wait when it is set. Note that the automatic drain runs in the
  plugin shutdown phase, which `.shutdown_grace_period(..)` does **not** bound
  (the grace period covers only the later tracked-task/`on_stop` phase) — so a
  hanging `create` with `create-timeout: 0`, or a `dispose` that never returns,
  stalls shutdown with no backstop. Keep a nonzero `create-timeout` (the default
  is 10s) unless you own that risk. Racing a manual `evict`/`invalidate` against
  `drain` is outside the invariant — the draining latch does not fence them. Outside a Tokio runtime it cannot spawn, so it drops the value without
  calling `dispose` and emits a debug log.
- There are no request leases. `get`, `Tenant<T>` and `Tenant<T>::into_inner`
  hand out clones, and eviction may dispose the shared resource while a clone is
  still held. Resource types must tolerate close-while-cloned. SQLx
  `Pool::close()` is graceful for already-acquired connections, but a clone that
  has not acquired a connection can receive `PoolClosed`. Disable idle eviction
  or use `.keep_forever()` when the resource cannot satisfy this contract.
- `max-active` is a soft cap maintained by a looping background trim and the
  periodic sweep, not an admission bound. Cold bursts can exceed it. The
  negative cache also inserts before bounding and may transiently overshoot
  `max-negative` under concurrent insertion: every inserting caller trims back
  toward the bound (with a bounded budget, so no caller loops on a moving
  target), and the periodic sweep purges expired entries on top. It converges
  back and never grows unbounded, but the bound is not guaranteed to hold at
  the instant any single call returns.

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
        .max_active(200))                                  // soft live-pool trim target
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

Steady-state connections are approximately `max_connections × live tenants`,
but `max_active` is a soft trim target, not admission control. A cold burst can
temporarily create more live pools, so `max_connections × max_active` is **not**
a safe hard capacity bound. Leave database headroom or add application-level
admission control when that bound matters.

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

---
topic: tenancy
features: tenant
tokens: ~3900
requires: di-beans, plugins
---

## Multi-Tenancy (per-tenant beans)

### TL;DR

- Requires feature `tenant` (crate `r2e-tenant`, re-exported as `r2e::tenant`): one process, many tenants, each with its own resource, created on first use and evicted when idle.
- You write two pieces — a `TenantResolver` (request → `TenantId`, or a built-in like `HeaderTenantResolver`) and a `TenantSource<T>` (tenant → `T`); the framework owns `Tenanted<T>`.
- Wire two plugins after providing those values: `Tenancy::resolver::<R>()` (provides `TenantRouter`) and `PerTenant::<T>::from::<Src>()` (provides `Tenanted<T>`).
- In a source, `Ok(None)` = unknown tenant and `Err` = source failure; `TenantError` maps missing 400 · unknown 404 · unavailable 503 · timeout 504 · cycle/no-resolver/no-source 500.
- Consume tenancy as **request-scoped controller fields** — `#[inject(request)] db: Tenant<PgPool>` / `tenant: TenantId` — not as handler parameters; neither implements axum's `FromRequestParts`, and a forgotten plugin is a compile error at `register_controller::<C>()`.
- `Tenant<T>` derefs to `T` and exposes `.tenant_id()`; an `Option<Tenant<T>>` / `Option<TenantId>` field means "no tenant", never "bad tenant".
- `TenantId::parse`/`parse_owned` validate `[a-z0-9][a-z0-9._-]{0,62}`, `from_static` panics on an invalid literal, and `TenantId` is `Serialize` but deliberately not `Deserialize`.
- A source may cascade onto the same tenant's other resources with `ctx.get::<U>().await?` and onto app-scoped beans with `ctx.bean::<U>()`; keep `create-timeout` nonzero, since concurrent cycles only surface as a 504.
- Config is `tenancy.*` (`enabled`, `on-missing`, `missing-status`, `max-active`, `idle-ttl`, `create-timeout`, `negative-ttl`), precedence `PerTenant` builder > `tenancy.*` > default; `max_active(0)` panics and `tenancy.enabled: false` is the off switch.
- There are no leases and `max-active` is a **soft** cap: a value can be disposed while a clone is held, and cold bursts exceed the target — never size a database from `max_connections × max_active`.

Requires feature: `tenant` (crate `r2e-tenant`, re-exported as `r2e::tenant`).

One process, many tenants, each with its **own** pool / client / cache
namespace — resolved from the request, created on first use, evicted when idle.
Three pieces:

| Piece | What it is | Who writes it |
|---|---|---|
| `TenantResolver` | request → `TenantId` | you, or a built-in (`HeaderTenantResolver`, …) |
| `TenantSource<T>` | tenant → `T` | you |
| `Tenanted<T>` | the map holding every tenant's `T` | the framework |

```rust
use std::time::Duration;

use r2e::tenant::{HeaderTenantResolver, PerTenant, Tenancy, Tenant, TenantContext,
                  TenantId, TenantSource, BoxError, BoxFuture};
use sqlx::PgPool;

// 1. how a tenant gets its pool
#[derive(Clone)]                            // every bean is Clone
pub struct PoolDirectory { directory: Directory }

impl TenantSource<PgPool> for PoolDirectory {
    fn create<'a>(&'a self, tenant: &'a TenantId, ctx: &'a TenantContext<'a>)
        -> BoxFuture<'a, Result<Option<PgPool>, BoxError>> {
        Box::pin(async move {
            let Some(url) = self.directory.dsn(tenant).await? else {
                return Ok(None);            // Ok(None) = unknown tenant → 404
            };
            Ok(Some(PgPool::connect(&url).await?))
        })
    }
    // optional: fn dispose(&self, tenant, value) -> BoxFuture<'_, ()>
}

// 2. wiring: two plugins
# fn __doc() -> impl Sized {
AppBuilder::new()
    .provide(HeaderTenantResolver::default())            // x-tenant-id
    .plugin(Tenancy::resolver::<HeaderTenantResolver>()) // provides TenantRouter
    .provide(PoolDirectory { directory })
    .plugin(PerTenant::<PgPool>::from::<PoolDirectory>() // provides Tenanted<PgPool>
        .max_active(200)
        .idle_ttl(Duration::from_secs(600)))
# }
```

`TenantId::parse` / `parse_owned` validate
`[a-z0-9][a-z0-9._-]{0,62}`. `TenantId::from_static(&'static str)` validates the
same grammar and **panics** on an invalid literal; it is convenience for trusted
static values, not an unchecked constructor. `TenantId` is `Serialize` but
deliberately not `Deserialize`.

Using it is a **request-scoped controller field** (`#[inject(request)]` is a
field attribute, not a handler-parameter one):

```rust
use r2e::tenant::{Tenant, TenantId};
use sqlx::PgPool;

#[controller(path = "/orders")]
struct OrderController {
    #[inject(request)] db: Tenant<PgPool>,        // this request's tenant's pool
    #[inject(request)] tenant: TenantId,          // just the id (provisions nothing)
    // Option<Tenant<T>> / Option<TenantId> = "no tenant" (never "bad tenant")
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> JsonResult<Vec<Order>> {
        Ok(Json(sqlx::query_as("select * from orders").fetch_all(&*self.db).await
            .map_err(|e| HttpError::internal(e.to_string()))?))
    }
}
# fn main() {}
```

`Tenant<T>` derefs to `T` and exposes `.tenant_id()`. Both extractors read beans
out of the HList state, so **forgetting a plugin is a compile error** at
`register_controller::<C>()`, not a 500 in production. Neither implements axum's
`FromRequestParts` (bridge-overlap invariant) — they are `#[inject(request)]`
fields only. The `Tenancy` layer installs a private `Arc<OnceCell<Option<TenantId>>>`
carrier in request extensions before routing. Guards, extractors and every
`#[managed]` acquisition share one resolver call, even on a managed-only route;
the cell stores the resolver's raw answer before missing-policy application,
memoizes `None`, and does not memoize errors. A bare `TenantId` extension is not
authoritative. `TenantRouter::memoized(&head)` is a read-only peek;
hand-wired routers without the plugin must call
`TenantRouter::install_memo(&mut extensions)` from their own pre-routing layer.

**Resolvers** (`TenantResolver`, async; `SyncTenantResolver` is the blanket-bridged
sync form): `HeaderTenantResolver::default()` (`x-tenant-id`) or `::new("x-org")`,
`PathTenantResolver::new("org")` (path param),
`ExtensionTenantResolver::<T, _>::new(|e| -> Option<TenantId> { ... })`
(default `Lenient` mode),
`ExtensionTenantResolver::<T, _, Strict>::try_new(|e| -> Result<Option<TenantId>, HttpError> { ... })`,
and `FnTenantResolver::new(|req: &RequestHead<'_>| ...)`. `Lenient` / `Strict`
are crate-root exports. `try_new` lets a present malformed claim return 400;
an absent extension is still `Ok(None)`. `Ok(None)` = "no tenant here" and
`tenancy.on-missing` decides. A JWT-claim or subdomain resolver is ~10 lines of
`SyncTenantResolver` — see the `resolver` module docs.

Extension/JWT resolution works with struct-level and parameter-level
`#[inject(identity)]`: generated handlers extract all `FromRequestParts`
parameters before snapshotting the head used by guards/managed resources. One
combination is unsupported: a controller-field `Tenant<T>` / `TenantId` whose
extension is populated by a parameter-level identity. Controller fields are
extracted first, so this fails closed as missing (or `None` under allow), never
as a wrong tenant. Move identity to the controller struct and use `#[anonymous]`
for its public routes.

**Cascade**: a source receives a `TenantContext` and can build on the *same*
tenant's other per-tenant resources — `ctx.get::<U>().await?` (lazy,
single-flighted, cycle-detected, names the chain on a cycle) and
`ctx.bean::<U>()` for app-scoped beans, plus `ctx.tenant()`.

Cycle detection is per resolution path: sequential A → B → A returns
`TenantError::Cycle`, but concurrent roots (task 1 creates A awaiting B while
task 2 creates B awaiting A) deadlock until `create-timeout` (504), or hang if
the timeout is disabled. Cycles are wiring bugs; keep `create-timeout` enabled
in production.

```yaml
tenancy:
  enabled: true          # false → app boots, nothing resolves (inert shells)
  on-missing: reject     # or `allow` (Option extractors then see None)
  missing-status: 400    # unknown-status: 404, unavailable-status: 503
  max-active: 500        # soft trim target per map, LRU beyond
  idle-ttl: 15m          # evict + dispose after this idleness (0 = never)
  create-timeout: 10s    # per `create` call; blowing it is a 504 (0 = none)
  negative-ttl: 5s       # how long an unknown tenant is remembered (0 = none)
```

Precedence: `PerTenant` builder > `tenancy.*` file > built-in default.
Builder: `.max_active(n)`, `.idle_ttl(d)`, `.keep_forever()`, `.create_timeout(d)`,
`.negative_ttl(d)`, `.eager([ids])` (preload at serve), `.fallback_to_default()`
(a tenant the source returns `Ok(None)` for gets the app-scoped `T` bean — which
is never disposed; adds `T` to the plugin's `Deps`).
`max-active` and `.max_active(n)` require `n >= 1`; zero panics during config
load/wiring/builder construction. Use `tenancy.enabled: false` as the off switch.

`TenantError` → status: missing 400 · unknown 404 · unavailable 503 · timeout 504
· cycle / no-resolver / no-source 500 (wiring bugs). The first three are
configurable.

Runtime API on the `Tenanted<T>` bean (`#[inject] map: Tenanted<PgPool>`):
`get(&id).await` (create-or-cached, single-flight), `peek(&id)`, `active()`,
`stats()`, `metrics()`, `evict(&id).await` (ready-only; awaits disposal;
`false` for an in-flight creation), `invalidate(&id)` (ready-only; `true` means
removed + disposal spawned, not awaited; outside Tokio it drops without
`dispose` and logs at debug), `sweep().await`, `preload(ids).await`,
`drain().await` (latches closed and repeatedly disposes ready snapshots;
in-flight creations self-dispose when they finish; later resolution is 503,
`the per-tenant resource map is draining (shutdown)`). `TenantedMetrics` and
`TenantStats` implement `Serialize`; `TenantStats::idle` serializes as whole-ms
`idle_ms`.

The removal APIs (`evict`, `invalidate`, sweeps, `drain`) only remove **ready**
slots, so they never detach an in-flight creation. The one removal that hits a
not-ready slot is the cleanup of a panicking/cancelled *initializer* (cancelling
a mere waiter touches nothing); the cell it detaches self-heals — a waiter that
inherits it and succeeds reattaches the slot, or, if the map is draining /
another resolve already recreated the key, disposes of the value instead. Every
**public** removal (`evict`/`invalidate`/sweep/`drain`) bumps a map-wide epoch
*before* taking the key's shard lock, and each initialization stamps the epoch it
started at on its **slot** (one reading shared by every participant on that cell,
not a per-caller capture): a *detached* completion from before a removal never
writes back — it neither reattaches its slot nor records a negative entry,
so `invalidate`/`evict` keep their immediacy — the value is disposed of and still
returned to its own caller. Both write-backs decide and write under the same
shard guard, which is what orders them against the removal. A vacant key cannot
distinguish "never existed" from "just invalidated", which is why vacancy alone
is not enough. The epoch is map-wide, so a removal may fence an unrelated
tenant's detached creation (cost: one rebuild); a still-mapped creation is never
fenced — it overlaps the removal, which never touched it, and is deliberately
left alone. A negative entry is likewise only ever written by the attempt
that still owns the key, so an "unknown" verdict can never shadow a live
resource; `resolve` also consults the ready slot **before** the negative cache.
One unknown cold wave makes one `create` call (negative cache re-check inside
initialization); errors are never cached, so an erroring wave may retry per
waiter. Negative-cache insertion then bounding may transiently exceed
`max-negative` under concurrency; every inserting caller trims back toward the
bound (bounded budget) and the sweep purges expired entries, so it converges and
never grows unbounded — but the bound may not hold at the instant a call
returns. Each
cached slot has a one-shot disposal gate; `TenantSource::dispose` is called at
most once per cached value — the gate commits first, so a `dispose` that panics
or is cancelled mid-await is not retried. The cleanup of a cancelled/panicking
initializer's *empty* slot deliberately does **not** bump the epoch (that would
forbid the legitimate waiter reattach), so two participants sharing a cell can
classify its value differently when a competing empty slot appears and then
vanishes. What makes that safe is where the gate is taken: whoever unmaps or
orphans a value commits its gate **inline, under the key's shard guard**, in the
same critical section — so either the orphan commits first and the other
participant reads `is_disposed()` under that same lock and refuses to restore, or
the restore lands first and the orphan finds its own slot back under the key and
keeps it. The rule holds with no exceptions: there is no gate CAS anywhere
outside a shard-lock critical section, removals (`take_ready`'s `remove_if`
predicate, `take_slot`'s entry guard) included, which is why `evict().await`
really has closed the resource when it returns: a participant arriving a moment
later loses the CAS instead of taking
the closing onto a detached task. A disposed value can never end up mapped, and
exactly one caller owes the `dispose` await. `drain()` returns only once
everything it is draining is **closed**: a live value can be outside the map and
still need closing (a resolve holding a slot detached under it, or a disposal
handed to a detached task), so both mint a counted in-flight guard — the disposal
one inside the same shard-lock critical section that took the gate — and `drain`
waits for that count to hit zero as well as for the map to empty. Only admitted
work counts: a resolve reads the draining latch *before* touching the counter, so
requests arriving after shutdown started are rejected without registering and a
flood of post-shutdown 503s cannot starve the drain. It therefore
also waits for an in-flight creation instead of letting it close itself behind
shutdown's back (bounded by `create-timeout` when set). The automatic drain runs
in the plugin shutdown phase, which `.shutdown_grace_period(..)` does NOT bound
— a hanging `create` with `create-timeout: 0` or a never-returning `dispose`
stalls shutdown with no backstop; keep a nonzero `create-timeout` (default 10s).
Racing a manual `evict`/`invalidate` against `drain` is outside the invariant.

There are **no leases**: `get`, `Tenant<T>` and `Tenant<T>::into_inner` return
clones that eviction may dispose while held. Resources must tolerate
close-while-cloned; SQLx `Pool::close()` lets acquired connections finish, but a
not-yet-acquired clone can get `PoolClosed`. Disable idle eviction / use
`.keep_forever()` if needed. `max-active` is a **soft cap** maintained by a
looping background trim plus periodic sweep, not admission control; cold bursts
can exceed it. The trim clears its one-at-a-time flag *before* re-checking, and
the re-check counts **ready** slots, so a creation that completed mid-round
(and therefore declined to schedule a trim of its own) is always picked up. It
stops when the ready count is back under the cap; `slots.len()` can stay over it
when the excess is still being created, which no trim can evict — that residual
is left to those creations completing and to the periodic sweep. Never use
`db max_connections × max_active` as a hard capacity bound.

Per-tenant SQLx / Diesel pools and their `#[managed]` transactions are the
same machinery applied to databases — see llm/tenancy-datasources.md.

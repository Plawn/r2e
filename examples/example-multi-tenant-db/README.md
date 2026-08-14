# example-multi-tenant-db

**Database-per-tenant.** One process, two tenants (`acme` and `globex`), each
with its **own SQLite file**. The isolation is physical: there is no
`WHERE tenant_id = ?` anywhere in this example, and no handler ever names a
tenant.

For the other model — one shared database, rows tagged with a tenant column,
isolation enforced by guards — see
[`examples/example-multi-tenant`](../example-multi-tenant/README.md).

## What it demonstrates

| Piece | Where | What it shows |
|---|---|---|
| `SyncTenantResolver` | `src/tenancy.rs` (`HeaderResolver`) | request → `TenantId`, from `x-tenant-id` |
| `PoolSource<Sqlite>` | `src/app.rs` | the framework's ready-made `TenantSource<Pool<Sqlite>>`: tenant → DSN → pool |
| `TenantTx<'_, Sqlite>` | `src/controllers.rs` (`NotesController`) | a `#[managed]` transaction on the tenant's own database |
| **Cascade** | `src/tenancy.rs` (`ApiClients`) | `ctx.get::<Pool<Sqlite>>()` — a per-tenant client built on the *same* tenant's per-tenant pool |
| **Fallback** | `src/tenancy.rs` (`Brandings`) | `.fallback_to_default()` — `Ok(None)` serves the app-scoped bean instead of a 404 |
| **Ops** | `src/controllers.rs` (`AdminController`) | `Tenanted<T>` as an ordinary `#[inject]` bean: `active()`, `stats()`, `metrics()`, `evict()`, `invalidate()` |

The master directory (`src/directory.rs`) is a SQLite `master.db` holding
`tenants(slug, dsn, api_token, theme)`. In production this is a Postgres table,
a control-plane API, or a config service — the shape the framework cares about
is the same: one lookup with three answers (provisioned / unknown / directory
down).

## Running

```bash
cargo run -p example-multi-tenant-db
```

Serves on `http://localhost:3000`. Each boot provisions a **fresh** data
directory under the system temp dir (printed at startup) and seeds it:

| Tenant | Notes | Custom theme | API token |
|---|---|---|---|
| `acme` | `Ship the beta`, `Order more anvils` | `acme-dark` | `acme-token-7f3` |
| `globex` | `Acquire a smaller company` | *(none — falls back)* | `globex-token-22a` |

## Routes

| Method | Path | Tenant? | Description |
|--------|------|---------|-------------|
| GET | `/notes` | required | This tenant's notes, read in a `TenantTx` |
| POST | `/notes` | required | Add a note — committed on this tenant's database only |
| POST | `/notes/rollback-demo` | required | Writes then fails; the insert must not survive |
| GET | `/whoami` | required | The resolved `TenantId` alone — provisions nothing |
| GET | `/client` | required | The cascaded `ApiClient`, plus a count queried through the cascaded pool |
| GET | `/branding` | required | Per-tenant `Branding`, or the shared default |
| GET | `/admin/tenants` | no | The directory: which tenants exist and where their data lives |
| GET | `/admin/pools` | no | Live per-tenant pools: `active`, `stats`, `metrics` |
| GET | `/admin/clients` | no | The same view for the cascaded client map |
| POST | `/admin/tenants/{tenant}/evict` | no | Drop **and dispose** a tenant's resources |
| POST | `/admin/tenants/{tenant}/invalidate` | no | Drop **now**, dispose in the background (e.g. the DSN changed) |
| GET | `/health` | no | Health check |

## Walkthrough

### 1. Two tenants, two databases

```bash
curl -H 'x-tenant-id: acme' localhost:3000/notes
# {"tenant":"acme","notes":["Ship the beta","Order more anvils"]}

curl -H 'x-tenant-id: globex' localhost:3000/notes
# {"tenant":"globex","notes":["Acquire a smaller company"]}
```

Nothing in `NotesController` names a tenant — it has **no fields at all**. The
`#[managed] tx: &mut TenantTx<'_, Sqlite>` parameter resolves the tenant, opens
(or reuses) that tenant's pool, and begins a transaction on it.

### 2. A write lands on one database only

```bash
curl -X POST -H 'x-tenant-id: acme' -H 'content-type: application/json' \
     -d '{"body":"Only acme can see this"}' localhost:3000/notes
# 201 {"tenant":"acme","notes":["Ship the beta","Order more anvils","Only acme can see this"]}

curl -H 'x-tenant-id: globex' localhost:3000/notes
# {"tenant":"globex","notes":["Acquire a smaller company"]}   ← untouched
```

And the rollback keeps its promise on the tenant's own database:

```bash
curl -X POST -H 'x-tenant-id: acme' localhost:3000/notes/rollback-demo
# 400 — the insert inside the handler is rolled back
curl -H 'x-tenant-id: acme' localhost:3000/notes
# ...still 3 notes, not 4
```

### 3. The two ways resolution fails

```bash
curl -i localhost:3000/notes
# 400 — no tenant in the request (`tenancy.on-missing: reject`)

curl -i -H 'x-tenant-id: ghost' localhost:3000/notes
# 404 — the directory has no such tenant (the source returned `Ok(None)`)

curl -i -H 'x-tenant-id: ACME' localhost:3000/notes
# 400 — not a valid TenantId; rejected by the resolver before any lookup
```

An unknown tenant leaves **no pool behind** and is negatively cached for
`negative-ttl` (5s here), so a stream of bad ids cannot hammer the directory:

```bash
curl -s localhost:3000/admin/pools
# {"active":["acme","globex"], ... ,"metrics":{...,"unknown":1,"negative":1,...}}
#            ↑ the two real tenants from steps 1–2; `ghost` is not among them
```

### 4. The cascade

`ApiClients::create` calls `ctx.get::<Pool<Sqlite>>().await?` — which resolves
**this same tenant's** pool through `PoolSource`, creating it on first use.

```bash
curl -H 'x-tenant-id: acme' localhost:3000/client
# {"tenant":"acme","token":"acme-token-7f3","notes_visible_through_the_cascaded_pool":3}

curl -H 'x-tenant-id: globex' localhost:3000/client
# {"tenant":"globex","token":"globex-token-22a","notes_visible_through_the_cascaded_pool":1}
```

The counts prove each client is holding its own tenant's database. One creation
each, in both maps — the client reused the pool rather than opening a second
one:

```bash
curl -s localhost:3000/admin/pools   | grep -o '"created":[0-9]*'   # "created":2
curl -s localhost:3000/admin/clients | grep -o '"created":[0-9]*'   # "created":2
```

### 5. The fallback

`Brandings` returns `Ok(None)` for a tenant with no `theme`, and the plugin was
built with `.fallback_to_default()`:

```bash
curl -H 'x-tenant-id: acme'   localhost:3000/branding
# {"tenant":"acme","branding":{"theme":"acme-dark","support_email":"support@acme.example"}}

curl -H 'x-tenant-id: globex' localhost:3000/branding
# {"tenant":"globex","branding":{"theme":"r2e-default","support_email":"support@example.com"}}

curl -H 'x-tenant-id: ghost'  localhost:3000/branding
# 200 with the default — fallback is per resource, not per app.
# The same `ghost` on /notes is still a 404.
```

### 6. Operating the maps

```bash
curl -s localhost:3000/admin/tenants
# [{"slug":"acme","dsn":"sqlite://...acme.db?mode=rwc","theme":"acme-dark"},
#  {"slug":"globex","dsn":"sqlite://...globex.db?mode=rwc","theme":null}]

curl -s localhost:3000/admin/pools
# {"active":["acme","globex"],
#  "stats":[{"tenant":"acme","ready":true,"idle_ms":812}, ...],
#  "metrics":{"active":2,"negative":0,"hits":7,"created":2,"create_failures":0,
#             "timeouts":0,"unknown":1,"fallbacks":0,"disposed":0,
#             "evicted_idle":0,"evicted_lru":0}}
```

**Evict** disposes: `PoolSource::dispose` closes the pool, releasing the
connections now rather than whenever the last handle drops. The client is
evicted first because it holds a clone of the pool.

```bash
curl -X POST localhost:3000/admin/tenants/acme/evict
# {"evicted_client":true,"evicted_pool":true}
curl -s localhost:3000/admin/pools | grep -o '"disposed":[0-9]*'   # "disposed":1
```

Eviction is a **cache** operation, not a deprovisioning — the next request
rebuilds:

```bash
curl -H 'x-tenant-id: acme' localhost:3000/notes   # 200, pool recreated
```

**Invalidate** is the "its DSN changed in the directory" shape: forget the
resource *synchronously* and close the old pool on a detached task, so the
caller never waits, in-flight requests finish on the old pool, and the next
request rebuilds from the new record.

```bash
curl -X POST localhost:3000/admin/tenants/globex/invalidate
# {"invalidated_client":true,"invalidated_pool":true}
curl -s localhost:3000/admin/pools | grep -o '"active":\[[^]]*\]'   # "active":["acme"]
curl -H 'x-tenant-id: globex' localhost:3000/notes                  # 200, rebuilt
```

## Tests

```bash
cargo test -p example-multi-tenant-db
```

`tests/tenancy_test.rs` boots the **same** `MultiTenantDbApp` via
`#[r2e::test(app = ...)]` — each test provisions its own throwaway data
directory. `.as_tenant("acme")` sets the header this app's resolver reads;
`.as_tenant_user(sub, tenant, roles)` is the variant that also mints a JWT
carrying a `tenant` claim, for apps whose resolver reads the identity instead.

## Configuration

`application.yaml` lists every `tenancy.*` knob at its default value. Anything
set on the `PerTenant` builder in `src/app.rs` wins over the file:

```text
builder setting  >  tenancy.* (file)  >  built-in default
```

This app caps the pools at `.max_active(16)` × `.max_connections(2)` = 32
connections, with a 5-minute idle TTL.

## See also

- [`docs/features/24-tenancy.md`](../../docs/features/24-tenancy.md) — the
  feature guide (resolvers, sources, cascade, fallback, config, ops).
- [`examples/example-multi-tenant`](../example-multi-tenant/README.md) — the
  row-level model: one database, a tenant column, guards enforcing isolation.

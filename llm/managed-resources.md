---
topic: managed-resources
features: data, data-sqlx | data-diesel
tokens: ~2900
requires: error-handling
---

## Managed Resources (Transactions)

### TL;DR

- Transactions are `#[managed]` handler params — the only transaction attribute (`#[transactional]` was removed).
- `use r2e::r2e_data_sqlx::Tx;` then `#[managed] tx: &mut Tx<'_, Sqlite>`; the pool is resolved from the bean graph by type, no `HasPool` impl.
- A custom resource implements BOTH `ManagedResource<S>` and `ManagedDeps` — there is no blanket impl. List the beans `acquire()` looks up in `type Deps`, or `type Deps = TNil;` when it reads none; a bean that was never provided is then a compile error at `register_controller()`.
- In `acquire`, take the request head with `context.require_request()?` (uniform 500 off-request) instead of unwrapping `ManagedContext::request`.
- `RequestHead<'a>` is the same `Copy` view guards see through `GuardContext::head()` — use its helpers (`header`, `path_param`, `host`, `extension::<T>()`).
- SSE and WebSocket handlers acquire no managed resources; every other HTTP handler shape does.
- Do not build `DbPool<DB>` by hand: install `SqlxDataSource<DB>` / `DieselDataSource<Conn>`, configure `datasource.*`, then `#[inject]` the pool or take `#[managed] tx: &mut DbTx<..>`.
- Attaching a migrator (`.migrations(&MIGRATOR)` / `embed_migrations!`) does not run it — only `migrate-at-start: true` does, inside `build_state()`, so a broken schema fails the boot and `TestApp` gets migrations too.
- A second datasource is a tag: `datasource_tag!(pub Reporting = "reporting")` mints the distinct bean type `DbPool<DB, Reporting>` configured under `datasource.reporting.*`.
- There is no `datasource.enabled` gate; to swap the database in a test pin the pool with `.override_bean(pool)` (neither the connection nor the migrations then run).

`#[managed]` params get `acquire()` before the handler and `release(success)`
after. `r2e-data-sqlx` ships a ready `Tx` — once the pool is provided, it just
works (the pool is resolved from the bean graph by type; no `HasPool` impl):

```rust
use r2e::r2e_data_sqlx::Tx;

# #[controller(path = "/users")]
# pub struct SqlxController {}
# #[routes]
# impl SqlxController {
#[post("/db")]
async fn create_in_db(&self, Json(body): Json<CreateUser>,
                      #[managed] tx: &mut Tx<'_, Sqlite>) -> JsonResult<User> {
    sqlx::query("INSERT INTO users (name) VALUES (?)")
        .bind(&body.name).execute(tx.as_mut()).await
        .map_err(|e| HttpError::internal(e.to_string()))?;
    Ok(Json(User { name: body.name }))
}
# }
# fn main() {}
```

Custom resources implement `ManagedResource<S>` (bound `S: BeanLookup` to pull
beans by type: `state.bean::<T>()`), error type `ManagedErr<HttpError>`.
`#[managed]` is the only transaction attribute (the legacy `#[transactional]`
body wrapper was removed).

Every `#[managed]` type must also implement `ManagedDeps`, listing the beans
`acquire()` looks up. There is no blanket impl: `#[routes]` folds each
`#[managed]` parameter type's `Deps` into the controller's dependency list, so a
bean that was never provided is a compile error at `register_controller()`
instead of a 500 on the first request. A resource that reads no bean says
`type Deps = TNil;`.

`ManagedContext<'_, S>` (passed to `acquire`) carries:

- `state: &S`, `controller: &'static str`, `handler: &'static str`
- `request: Option<RequestHead<'_>>` — the incoming request head, `Some` on HTTP
  routes, `None` off-request. Use `context.require_request()?` (uniform 500
  naming `controller::handler`) rather than unwrapping.
- `missing_bean(prefix, bean, hint) -> ManagedErr<HttpError>` — uniform
  "bean not provided" error.

`RequestHead<'a>` is a `Copy` bundle of borrows: fields `method: &Method`,
`uri: &Uri`, `headers: &HeaderMap`, `extensions: &Extensions`,
`path_params: PathParams<'a>`, `peer_addr: Option<SocketAddr>`; helpers
`path()`, `query_string()`, `header(name)`, `path_param(name)`, `host()`
(`Host` header, else URI authority), `extension::<T>()`. Guards see the same
view through `GuardContext::head()`.

```rust
use r2e::{ManagedContext, ManagedDeps, ManagedErr, ManagedOutcome, ManagedResource, TNil};

pub struct TenantAudit { tenant: String }

impl<S: Send + Sync> ManagedResource<S> for TenantAudit {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let head = context.require_request()?;              // 500 off-request
        let tenant = head.header("x-tenant")
            .or_else(|| head.path_param("tenant"))
            .ok_or_else(|| ManagedErr(HttpError::BadRequest("tenant missing".into())))?;
        Ok(Self { tenant: tenant.to_string() })
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> { Ok(()) }
    fn abort(&mut self) {}
}

impl ManagedDeps for TenantAudit {
    type Deps = TNil;                                       // reads no bean
}
# fn main() {}
```

The head is available on every HTTP handler shape (plain, `#[guard]`ed,
`#[intercept]`ed, and `#[anonymous]` routes on an identity controller). SSE and
WebSocket handlers do not acquire managed resources.

For `r2e-data-sqlx`/`r2e-data-diesel`, `TxSource` declares which pool bean its
transactions need — `type Deps` is `TCons<Pool<DB>, TNil>` for `FixedPool`
(`Tx`) and `TCons<DbPool<DB>, TNil>` for `RotatingPool` (`DbTx`) — and
`ManagedTx` forwards it (`type Deps = Src::Deps`). Using `Tx` without providing
the pool no longer compiles.

### The datasource plugin (`SqlxDataSource` / `DieselDataSource`)

`DbPool<DB>` is not produced by hand: install the datasource plugin and it
connects the pool from config, optionally migrates, starts the live-URL
rotation loop at serve time, and closes the pool on graceful shutdown.

```rust
use r2e::r2e_data_sqlx::{DbPool, DbTx, SqlxDataSource};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

# fn __doc() -> impl Sized {
AppBuilder::new()
    .load_config::<()>()                    // provides LiveConfigRegistry (the plugin's dep)
    .plugin(SqlxDataSource::<sqlx::Postgres>::new().migrations(&MIGRATOR))
    .register::<ArticleService>()           // #[inject] pool: DbPool<sqlx::Postgres>
# }

// ... and the transaction is requested as before:
# #[controller(path = "/articles")]
# pub struct ArticleController {}
# #[routes]
# impl ArticleController {
#[post("/db")]
async fn create_in_db(&self,
                      #[managed] tx: &mut DbTx<'_, sqlx::Postgres>) -> JsonResult<User> {
    sqlx::query("INSERT INTO users (name) VALUES (?)")
        .bind("Ada").execute(tx.as_mut()).await
        .map_err(|e| HttpError::internal(e.to_string()))?;
    Ok(Json(User { name: "Ada".into() }))
}
# }
# fn main() {}
```

```yaml
datasource:
  url: "postgres://user:pass@localhost/app"   # read live: the pool rotates on change
  max-connections: 20                         # optional, SQLx default
  min-connections: 2                          # optional, SQLx default
  acquire-timeout: 10s                        # optional, SQLx default
  migrate-at-start: true                      # default false
```

`migrate-at-start` (Quarkus' `quarkus.flyway.migrate-at-start`) is the only
thing that runs the migrator attached with `.migrations(&MIGRATOR)` — attaching
is not running, so the same binary migrates in dev and stays read-only in
production. Migrations run **inside `build_state()`**, so a broken schema fails
the boot (`Plugin 'SqlxDataSource' failed to build: ...`) and `TestApp` gets
them too — which the old serve-only `.on_start(|state| migrate)` never did.
`migrate-at-start: true` with no attached migrator only warns.

A **named** datasource is a second tag: `datasource_tag!(pub Reporting =
"reporting")` mints a marker whose config lives under `datasource.reporting.*`
and whose bean is `DbPool<DB, Reporting>` — a distinct type, so the default and
the named pool coexist and inject unambiguously. `DbPool<DB>` is
`DbPool<DB, DefaultDataSource>`; `DbTx<'_, DB, Tag>` follows the same default.

There is **no** `datasource.enabled` gate (a pool bean has no inert form —
setting it only warns). To swap the database in a test, pin the pool with
`.override_bean(pool)`: the plugin sets `SKIP_BUILD_WHEN_ALL_PINNED = true`, so
neither the connection nor the migrations run.

`DieselDataSource<Conn>` is the exact mirror — same `datasource.*` section, same
`migrate-at-start` gate — taking `diesel_migrations::embed_migrations!()`:

```rust
use r2e::r2e_data_diesel::DieselDataSource;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");
# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(DieselDataSource::<PgConnection>::new().migrations(&MIGRATIONS))
# }
# fn main() {}
```

r2d2's build and Diesel's migration harness are blocking, so both run on the
blocking pool; r2d2 pools have no async close, so the plugin registers no
shutdown hook (dropping the last handle is the close).

`DbPool` connects a replacement pool when `datasource.url` changes, swaps only
after the new connection succeeds, increments `generation()`, and closes the
old pool in the background. In-flight `DbTx` values keep using the generation
they acquired.

The pool and its generation are published as one atomic snapshot, so they can
never be read out of step: `snapshot() -> (Pool<DB>, u64)` returns the pair
(`current()` and `generation()` remain, for when only one of them matters).
Because the old pool is closed right after the swap, a snapshot taken just
before a rotation can hit `sqlx::Error::PoolClosed`; `DbPool::begin() ->
Result<(Transaction<'static, DB>, u64), sqlx::Error>` and the
`Executor for &DbPool` impl re-read the snapshot and retry (bounded to three
attempts), so a rotation never surfaces as a 500. `DbTx::generation()` always
reports the pool the transaction actually ran on.

`r2e-data-diesel` mirrors the same shape for r2d2 pools: `Tx`/`DieselTx<Conn>`
on a provided `Pool<ConnectionManager<Conn>>` bean, and `DbPool<Conn>` +
`DbTx<Conn>` (no lifetime param) for rotating credentials. Diesel's pools build
on a blocking thread, so `DbPool::connect_with` takes a pool-factory closure
instead of pool options; rotation drops the facade's handle on the old pool,
which stays alive (and usable) until its last handle/connection goes away —
r2d2 pools are never explicitly closed, so there is no closed-pool window and
no retry. It exposes the same atomic
`snapshot() -> (Pool<ConnectionManager<Conn>>, u64)`.

Diesel's `TxSource::acquire_pool` is **async** (`fn acquire_pool<S>(&ManagedContext) ->
impl Future<Output = Result<(Pool<ConnectionManager<Conn>>, Self::Meta), ManagedErr<HttpError>>> + Send`):
a per-tenant source may have to *create* the tenant's pool there, which is
network-bound. Custom sources written against the old blocking signature just
become `async fn`. `TxSource::Meta` is `Clone + Send + Sync + 'static` (was
`Copy`), and `ManagedTx::meta()` is the generic accessor behind
`generation()` / `tenant()`.

```rust
use r2e::r2e_data_diesel::{DbPool, DbTx, DieselDataSource};

// b.plugin(DieselDataSource::<SqliteConnection>::new())  provides DbPool<SqliteConnection>

# #[controller(path = "/diesel")]
# pub struct DieselController {}
# #[routes]
# impl DieselController {
#[post("/db")]
async fn create_in_db(&self, #[managed] tx: &mut DbTx<SqliteConnection>) -> JsonResult<User> {
    let new = NewUser { name: "Ada".into() };
    tx.run(move |c| diesel::insert_into(users::table).values(&new).execute(c)).await?;
    Ok(Json(User { name: "Ada".into() }))
}
# }
# fn main() {}
```

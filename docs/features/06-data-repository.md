# Feature 6 — Pagination and Managed Transactions

## TL;DR

`Pageable` and `Page<T>` live in `r2e-core` and are exported by the prelude with no feature flag — the old generic `Entity` / `Repository` / `QueryBuilder` / `DataError` layer was removed (unused, duplicated the query APIs apps already use). Database support is intentionally limited to R2E's managed transaction lifecycles: `sqlx-{sqlite,postgres,mysql}` and `diesel-{sqlite,postgres,mysql}`. Connecting is a plugin — `.plugin(SqlxDataSource::<Postgres>::new().migrations(&MIGRATOR))` reads the `datasource.*` section, connects, migrates when `migrate-at-start` is on, and closes the pool at shutdown. See the book's managed-transaction guide for setup and route examples.


## Pagination: `Pageable` and `Page<T>`

`Pageable` is a `#[derive(Params)]` struct with three zero-based query
parameters:

- `page` (default `0`)
- `size` (default `20`)
- `sort` (`Option<String>`)

`offset()` returns `page * size`. `Page::new(content, &pageable, total_elements)`
builds the response and computes `total_pages`; a `size` of `0` gives `0`
pages.

```rust,ignore
use r2e::prelude::*;

#[get("/")]
async fn list(&self, Query(pageable): Query<Pageable>) -> Json<Page<User>> {
    let total = self.users.count().await;
    let rows = self.users.list(pageable.size, pageable.offset()).await;
    Json(Page::new(rows, &pageable, total))
}
```

`GET /users?page=1&size=10` serializes as:

```json
{ "content": [ ... ], "page": 1, "size": 10, "total_elements": 42, "total_pages": 5 }
```

`Pageable` also nests inside your own `Params` struct. With
`#[params] pageable: Pageable` its fields stay flat. With
`#[params(prefix)]` or `#[params(prefix = "p")]` they get a prefix. The
OpenAPI spec then documents `page`, `size` and `sort` as optional query
parameters.

## Managed transactions

Database integrations are intentionally limited to R2E-specific managed
transaction lifecycles:

- `sqlx-sqlite`, `sqlx-postgres`, `sqlx-mysql`;
- `diesel-sqlite`, `diesel-postgres`, `diesel-mysql`.

## The datasource plugin

Neither backend expects a hand-built pool. `SqlxDataSource<DB, Tag>` and
`DieselDataSource<Conn, Tag>` are plugins that own a database's whole
boot:

```rust
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

AppBuilder::new()
    .load_config::<()>()
    .plugin(SqlxDataSource::<sqlx::Postgres>::new().migrations(&MIGRATOR))
```

```yaml
datasource:
  url: "postgres://user:pass@localhost/app"
  max-connections: 20
  min-connections: 2
  acquire-timeout: 10s
  migrate-at-start: true    # default false
```

- provides `DbPool<DB>` (rotating: the URL is read as a live value, and the
  pool swaps onto a new one without dropping in-flight transactions);
- runs the attached migrations when `migrate-at-start` is true — inside
  `build_state()`, so a broken migration fails the boot and tests get the
  schema too;
- starts the rotation watcher at serve time and closes the pool on graceful
  shutdown.

A named datasource — `datasource_tag!(pub Reporting = "reporting")` — reads
`datasource.reporting.*` and provides `DbPool<DB, Reporting>`, a distinct bean
type, so several databases coexist in one app. There is no `datasource.enabled`
flag: to replace the pool in a test, pin it with `override_bean`, which skips
the connection and the migrations entirely.

See the book's [managed database transaction guide](../book/src/data-access/transactions.md)
for setup and route examples.

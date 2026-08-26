# Pagination and managed transactions

## TL;DR

`Pageable` and `Page<T>` live in `r2e-core` and are exported by the prelude with no feature flag — the old generic `Entity` / `Repository` / `QueryBuilder` / `DataError` layer was removed (unused, duplicated the query APIs apps already use). Database support is intentionally limited to R2E's managed transaction lifecycles: `sqlx-{sqlite,postgres,mysql}` and `diesel-{sqlite,postgres,mysql}`. Connecting is a plugin — `.plugin(SqlxDataSource::<Postgres>::new().migrations(&MIGRATOR))` reads the `datasource.*` section, connects, migrates when `migrate-at-start` is on, and closes the pool at shutdown. See the book's managed-transaction guide for setup and route examples.


The former generic `Entity`, `Repository`, `QueryBuilder`, and `DataError`
layer has been removed. It was not consumed by either backend and duplicated
the query APIs applications already use directly.

`Pageable` and `Page<T>` now live in `r2e-core` and are exported by
`r2e::prelude::*` without a feature flag.

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

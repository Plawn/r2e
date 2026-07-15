# Managed database transactions

R2E provides cancellation-safe request transactions for SQLx and Diesel. A
transaction is acquired before the route runs, committed for responses below
status 400, and rolled back for `4xx`/`5xx` responses. Panic, cancellation, and
partial acquisition use a synchronous abort fallback.

## SQLx

Enable one driver and add SQLx for its query API:

```toml
[dependencies]
r2e = { version = "0.1", features = ["sqlx-postgres"] }
sqlx = { version = "0.9", features = ["runtime-tokio", "postgres"] }
```

Provide the pool as a bean:

```rust
let pool = sqlx::PgPool::connect(&database_url).await?;

AppBuilder::new()
    .provide(pool)
    .build_state()
    .await;
```

Then request a transaction explicitly:

```rust
use r2e::prelude::*;
use r2e::r2e_data_sqlx::Tx;
use sqlx::Postgres;

#[post("/users")]
async fn create(
    &self,
    Json(body): Json<CreateUser>,
    #[managed] tx: &mut Tx<'_, Postgres>,
) -> Result<StatusCode, HttpError> {
    sqlx::query("INSERT INTO users(name) VALUES ($1)")
        .bind(body.name)
        .execute(tx.connection())
        .await
        .map_err(|error| HttpError::internal(error.to_string()))?;

    Ok(StatusCode::CREATED)
}
```

Available façade features are `sqlx-sqlite`, `sqlx-postgres`, and
`sqlx-mysql`. The shorter `sqlite`, `postgres`, and `mysql` names remain
compatibility aliases for SQLx.

## Diesel

Enable the matching Diesel backend:

```toml
[dependencies]
r2e = { version = "0.1", features = ["diesel-postgres"] }
diesel = { version = "2", features = ["postgres", "r2d2"] }
```

Provide an r2d2 pool:

```rust
use diesel::{PgConnection, r2d2::{ConnectionManager, Pool}};

let manager = ConnectionManager::<PgConnection>::new(database_url);
let pool = Pool::builder().build(manager)?;

AppBuilder::new()
    .provide(pool)
    .build_state()
    .await;
```

Diesel is synchronous. Use `tx.run(...)` to execute its work on Tokio's
blocking pool without blocking an async worker:

```rust
use diesel::prelude::*;
use r2e::prelude::*;
use r2e::r2e_data_diesel::DieselTx;

#[post("/users")]
async fn create(
    &self,
    Json(body): Json<CreateUser>,
    #[managed] tx: &mut DieselTx<PgConnection>,
) -> Result<StatusCode, HttpError> {
    tx.run(move |connection| {
        diesel::insert_into(users::table)
            .values((users::name.eq(body.name), users::email.eq(body.email)))
            .execute(connection)
    })
    .await?;

    Ok(StatusCode::CREATED)
}
```

The Diesel features are `diesel-sqlite`, `diesel-postgres`, and
`diesel-mysql`. `tx.connection()` is also available when code is already on a
blocking thread. Diesel's MySQL driver additionally requires a compatible
native `libmysqlclient`/MariaDB client library at build time.

## Lifecycle and response policy

For each request R2E performs:

1. acquire resources in handler parameter order;
2. run the handler and build its HTTP response;
3. classify `< 400` as success and `4xx`/`5xx` as failure;
4. finalize every resource in reverse order;
5. return a finalization error if commit or rollback failed.

Already acquired resources abort if a later acquisition fails. A panic or
cancelled request drops the managed guard and invokes the same abort fallback.
SQLx drops its unfinished transaction; Diesel discards an r2d2 connection that
still owns an open transaction.

`#[managed]` and the legacy `#[transactional]` decorator are mutually
exclusive. Prefer `#[managed]`: it makes the transaction used by each query
explicit and provides cancellation-safe cleanup.

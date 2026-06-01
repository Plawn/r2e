# r2e-data-sqlx

[SQLx](https://github.com/launchbadge/sqlx) backend for the R2E data layer — SqlxRepository, transactions, managed resources, and migrations.

## Overview

Provides a concrete `SqlxRepository` implementation of the `Repository` trait from [`r2e-data`](../r2e-data), along with transaction support via managed resources.

## Usage

Via the facade crate with a database driver:

```toml
[dependencies]
r2e = { version = "0.1", features = ["sqlite"] }
# or "postgres", "mysql"
```

## Feature flags

| Feature | Driver |
|---------|--------|
| `sqlite` | SQLite via `sqlx/sqlite` |
| `postgres` | PostgreSQL via `sqlx/postgres` |
| `mysql` | MySQL via `sqlx/mysql` |

## Key types

### SqlxRepository

SQLx-backed implementation of the `Repository` trait:

```rust
use r2e::r2e_data_sqlx::SqlxRepository;

let repo = SqlxRepository::<User, Sqlite>::new(pool.clone());
let users = repo.find_all().await?;
```

### Transactions with `#[managed]`

```rust
use r2e::r2e_data_sqlx::Tx;

#[post("/")]
async fn create(
    &self,
    body: Json<CreateUser>,
    #[managed] tx: &mut Tx<'_, Sqlite>,
) -> Result<Json<User>, HttpError> {
    sqlx::query("INSERT INTO users (name) VALUES (?)")
        .bind(&body.name)
        .execute(tx.as_mut())
        .await?;
    Ok(Json(user))
}
```

The transaction is automatically committed on success or rolled back on error.

### HasPool

Trait for extracting a database pool from your application state:

```rust
use r2e::r2e_data_sqlx::HasPool;

impl HasPool<Sqlite> for AppState {
    fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
```

## License

Apache-2.0

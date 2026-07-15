# r2e-data-sqlx

Cancellation-safe managed SQLx transactions for R2E. This crate intentionally
contains no repository, entity, query-builder, migration, or generic data
abstraction.

```toml
[dependencies]
r2e = { version = "0.1", features = ["sqlx-sqlite"] }
sqlx = { version = "0.9", features = ["runtime-tokio", "sqlite"] }
```

Register `Pool<DB>` with `.provide(pool)`, then use:

```rust
#[post("/items")]
async fn create(
    &self,
    #[managed] tx: &mut r2e::r2e_data_sqlx::Tx<'_, sqlx::Sqlite>,
) -> Result<StatusCode, HttpError> {
    sqlx::query("INSERT INTO items(name) VALUES (?)")
        .bind("item")
        .execute(tx.connection())
        .await
        .map_err(|error| HttpError::internal(error.to_string()))?;
    Ok(StatusCode::CREATED)
}
```

Responses below 400 commit; `4xx`/`5xx` responses roll back. Panic and
cancellation fall back to SQLx transaction drop rollback.

Features: `sqlite`, `postgres`, `mysql`.

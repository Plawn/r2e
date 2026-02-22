# Transactions and Managed Resources

R2E provides two approaches for transaction management: `#[managed]` for automatic lifecycle and `#[transactional]` for simple wrapping.

## `#[managed]` — Recommended

The `#[managed]` attribute provides automatic resource lifecycle management:

```rust
use r2e::r2e_data_sqlx::Tx;
use sqlx::Sqlite;

#[post("/")]
async fn create(
    &self,
    body: Json<CreateUserRequest>,
    #[managed] tx: &mut Tx<'_, Sqlite>,
) -> Result<Json<User>, AppError> {
    sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
        .bind(&body.name)
        .bind(&body.email)
        .execute(tx.as_mut())
        .await?;

    Ok(Json(user))
}
```

### Lifecycle

1. **Acquire**: `Tx::acquire(&state)` — begins a transaction from the pool
2. **Handler runs**: receives `&mut Tx` — executes queries within the transaction
3. **Release**: `Tx::release(self, success)` — commits on success, rolls back on failure
   - `success = true` if handler returned `Ok` or a non-Result type
   - `success = false` if handler returned `Err`

### Requirements

Your state must implement `HasPool`:

```rust
use r2e::r2e_data_sqlx::HasPool;

impl HasPool<Sqlite> for AppState {
    fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}
```

## `#[transactional]` — Simple wrapping

For basic transaction wrapping without explicit `Tx` parameter:

```rust
#[post("/")]
#[transactional]
async fn create(&self, body: Json<CreateUserRequest>) -> Result<Json<User>, AppError> {
    // self.pool is automatically wrapped in begin()/commit()
    sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
        .bind(&body.name)
        .bind(&body.email)
        .execute(&self.pool)
        .await?;

    Ok(Json(user))
}
```

Use `#[transactional(pool = "custom_pool")]` if your pool field has a different name.

> **Note:** `#[managed]` and `#[transactional]` are mutually exclusive. Prefer `#[managed]` for new code — it's more explicit and flexible.

## Custom managed resources

Implement `ManagedResource<S>` for any type that needs acquire/release lifecycle:

```rust
use r2e::prelude::*; // ManagedResource, ManagedErr

pub struct Tx<'a, DB: Database>(pub Transaction<'a, DB>);

impl<S, DB> ManagedResource<S> for Tx<'static, DB>
where
    DB: Database,
    S: HasPool<DB> + Send + Sync,
{
    type Error = ManagedErr<MyAppError>;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state.pool().begin().await
            .map_err(|e| ManagedErr(MyAppError::Database(e.to_string())))?;
        Ok(Tx(tx))
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        if success {
            self.0.commit().await
                .map_err(|e| ManagedErr(MyAppError::Database(e.to_string())))?;
        }
        // On failure: transaction dropped → automatic rollback
        Ok(())
    }
}
```

### Error wrappers

`ManagedResource::Error` must implement `Into<Response>`. Use `ManagedErr<E>` to wrap your custom error type:

```rust
// Your error type
impl IntoResponse for MyAppError { /* ... */ }

// ManagedResource uses the wrapper
type Error = ManagedErr<MyAppError>;
```

The chain is: `MyAppError` → `ManagedErr<MyAppError>` → `Response`.

## Other managed resource ideas

The pattern isn't limited to transactions:

- **Audit context** — acquire logs "action started", release logs "action completed"
- **Scoped cache** — acquire creates a request-scoped cache, release flushes it
- **Connection checkout** — acquire checks out a connection, release returns it

# Managed Resources

The `#[managed]` attribute enables automatic lifecycle management for resources that need acquire/release semantics — transactions, connections, scoped caches, or audit contexts.

## The `ManagedResource` trait

```rust
pub trait ManagedResource<S>: Sized {
    type Error: Into<Response>;

    async fn acquire(state: &S) -> Result<Self, Self::Error>;
    async fn release(self, success: bool) -> Result<(), Self::Error>;
}
```

- `acquire()` — called before the handler, obtains the resource from app state
- `release()` — called after the handler, commits or rolls back
  - `success = true` — handler returned `Ok` or a non-Result type
  - `success = false` — handler returned `Err`

## Usage

```rust
#[post("/")]
async fn create(
    &self,
    body: Json<CreateUserRequest>,
    #[managed] tx: &mut Tx<'_, Sqlite>,
) -> Result<Json<User>, MyAppError> {
    sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
        .bind(&body.name)
        .bind(&body.email)
        .execute(tx.as_mut())
        .await?;

    Ok(Json(user))
}
```

## Implementing for transactions

```rust
use r2e::prelude::*; // ManagedResource, ManagedErr
use r2e::r2e_data_sqlx::HasPool;
use sqlx::{Database, Transaction, Pool};

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

## Error wrappers

`ManagedResource::Error` must implement `Into<Response>`. Rust's orphan rules prevent implementing foreign traits for foreign types, so R2E provides wrappers:

- `ManagedError` — wraps the built-in `AppError`
- `ManagedErr<E>` — wraps any error type implementing `IntoResponse`

```rust
// Chain: MyAppError → ManagedErr<MyAppError> → Response
type Error = ManagedErr<MyAppError>;
```

## Other resource types

The pattern extends beyond transactions:

### Audit context

```rust
pub struct AuditContext {
    started_at: Instant,
    action: String,
}

impl<S: Send + Sync> ManagedResource<S> for AuditContext {
    type Error = ManagedError;

    async fn acquire(_state: &S) -> Result<Self, Self::Error> {
        Ok(AuditContext {
            started_at: Instant::now(),
            action: String::new(),
        })
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        let duration = self.started_at.elapsed();
        tracing::info!(
            action = self.action,
            success,
            duration_ms = duration.as_millis(),
            "Audit: action completed"
        );
        Ok(())
    }
}
```

## Comparison with `#[transactional]`

| Feature | `#[managed]` | `#[transactional]` |
|---------|-------------|-------------------|
| Explicit parameter | Yes (`&mut Tx`) | No (wraps `self.pool`) |
| Custom resource types | Yes | No (transactions only) |
| Error handling | Configurable via trait | Fixed |
| Flexibility | High | Convenient |

Prefer `#[managed]` for new code.

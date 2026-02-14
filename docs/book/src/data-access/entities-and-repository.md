# Entities and Repository

R2E provides data access abstractions: `Entity` for mapping Rust structs to SQL tables, and `Repository` for async CRUD operations.

## Setup

Enable the data feature:

```toml
r2e = { version = "0.1", features = ["data"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
```

## Defining entities

Implement the `Entity` trait to map a struct to a database table:

```rust
use r2e::r2e_data::Entity;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl Entity for User {
    type Id = i64;

    fn table_name() -> &'static str {
        "users"
    }

    fn id_column() -> &'static str {
        "id"
    }

    fn columns() -> &'static [&'static str] {
        &["id", "name", "email"]
    }

    fn id(&self) -> &i64 {
        &self.id
    }
}
```

### Entity trait methods

| Method | Returns | Description |
|--------|---------|-------------|
| `table_name()` | `&'static str` | SQL table name |
| `id_column()` | `&'static str` | Primary key column name |
| `columns()` | `&'static [&'static str]` | All column names |
| `id(&self)` | `&Id` | Primary key value |

## Using `SqlxRepository`

`SqlxRepository` provides CRUD operations backed by SQLx:

```rust
use r2e::r2e_data_sqlx::SqlxRepository;

let repo = SqlxRepository::<User, _>::new(pool.clone());

// Find by ID
let user: Option<User> = repo.find_by_id(&1).await?;

// Find all
let users: Vec<User> = repo.find_all().await?;

// Create
let user = repo.create(&new_user).await?;

// Update
repo.update(&updated_user).await?;

// Delete
repo.delete(&1).await?;
```

## In controllers

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] pool: SqlitePool,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Result<Json<Vec<User>>, AppError> {
        let users = sqlx::query_as::<_, User>("SELECT id, name, email FROM users")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(Json(users))
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<User>, AppError> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, name, email FROM users WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("User not found".into()))?;

        Ok(Json(user))
    }
}
```

## HasPool trait

For controllers that use data-sqlx features, implement `HasPool` on your state:

```rust
use r2e::r2e_data_sqlx::HasPool;
use sqlx::{Pool, Sqlite};

impl HasPool<Sqlite> for AppState {
    fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}
```

This is required for `#[managed]` transactions and `SqlxRepository`.

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
#[controller(path = "/users")]
pub struct UserController {
    #[inject] pool: SqlitePool,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Result<Json<Vec<User>>, HttpError> {
        let users = sqlx::query_as::<_, User>("SELECT id, name, email FROM users")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| HttpError::Internal(e.to_string()))?;
        Ok(Json(users))
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<User>, HttpError> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, name, email FROM users WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?
        .ok_or_else(|| HttpError::NotFound("User not found".into()))?;

        Ok(Json(user))
    }
}
```

## Making the pool available

Data-sqlx features (`#[managed]` transactions and `SqlxRepository`) fetch the
database pool from the bean graph **by type**. There is no state trait to
implement — just make the pool a bean by providing it before `build_state()`:

```rust
use sqlx::{Pool, Sqlite};

let pool: Pool<Sqlite> = SqlitePool::connect(&url).await?;

AppBuilder::new()
    .provide(pool)     // `Pool<Sqlite>` is now resolvable by type
    // ...
    .build_state()
    .await
    // ...
```

Once `Pool<Sqlite>` is provided, controllers can `#[inject] pool: SqlitePool`
and `#[managed] tx: &mut Tx<'_, Sqlite>` resolves the same pool automatically.

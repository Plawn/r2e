# r2e-data

Data access abstractions for R2E — Entity, Repository, QueryBuilder, Page, and Pageable. Driver-independent.

## Overview

`r2e-data` defines the data access traits and types used across R2E applications. It has **no database driver dependencies**, making it suitable for defining interfaces in your domain layer. Use [`r2e-data-sqlx`](../r2e-data-sqlx) or [`r2e-data-diesel`](../r2e-data-diesel) for concrete implementations.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["data"] }
```

## Key types

### Entity

Maps a Rust struct to a SQL table:

```rust
use r2e::r2e_data::{Entity};

impl Entity for User {
    fn table_name() -> &'static str { "users" }
    fn columns() -> &'static [&'static str] { &["id", "name", "email"] }
}
```

### Repository

Async CRUD interface:

```rust
use r2e::r2e_data::Repository;

// find_by_id, find_all, create, update, delete
let user = repo.find_by_id(1).await?;
let users = repo.find_all().await?;
```

### QueryBuilder

Fluent SQL query builder:

```rust
use r2e::r2e_data::QueryBuilder;

let query = QueryBuilder::select::<User>()
    .where_eq("active", true)
    .where_like("name", "%john%")
    .order_by("created_at", false)
    .limit(10)
    .offset(0)
    .build();
```

### Pagination

```rust
use r2e::r2e_data::{Page, Pageable};

// Pageable is extracted from query string: ?page=0&size=20&sort=name,asc
#[get("/")]
async fn list(&self, pageable: Pageable) -> Result<Json<Page<User>>, AppError> {
    Ok(Json(self.service.list(pageable).await?))
}
```

### DataError

Standard error type for data layer operations, bridged to `AppError` for HTTP responses.

## License

Apache-2.0

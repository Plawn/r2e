# Feature 6 — Data / Repository

## Objective

Provide abstractions for data access: the `Entity` trait for modeling tables, `QueryBuilder` for building SQL queries in a fluent manner, and `Pageable`/`Page<T>` for pagination.

## Key Concepts

### Entity

A trait representing a persisted entity in the database. It defines the table name, columns, and the identifier column.

### QueryBuilder

A fluent builder for constructing SELECT queries with conditions, sorting, and pagination.

### Pageable / Page\<T\>

`Pageable` is a struct extractable from query params for pagination. `Page<T>` is the paginated response container.

## Usage

### 1. Add the dependency

```toml
[dependencies]
r2e-data = { path = "../r2e-data", features = ["sqlite"] }
```

### 2. Define an entity

```rust
use r2e_data::Entity;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct UserEntity {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl Entity for UserEntity {
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

### 3. Use QueryBuilder

```rust
use r2e_data::QueryBuilder;

// Simple query
let (sql, params) = QueryBuilder::new("users")
    .build_select("*");
// → "SELECT * FROM users", []

// Query with conditions
let (sql, params) = QueryBuilder::new("users")
    .where_eq("email", "alice@example.com")
    .where_like("name", "%ali%")
    .order_by("id", true)
    .limit(10)
    .offset(20)
    .build_select("id, name, email");
// → "SELECT id, name, email FROM users WHERE email = ? AND name LIKE ? ORDER BY id ASC LIMIT 10 OFFSET 20"
// → params: ["alice@example.com", "%ali%"]

// COUNT query
let (sql, params) = QueryBuilder::new("users")
    .where_eq("active", "true")
    .build_count();
// → "SELECT COUNT(*) FROM users WHERE active = ?"
```

### Available condition methods

| Method | Generated SQL | Example |
|--------|--------------|---------|
| `where_eq(col, val)` | `col = ?` | `.where_eq("status", "active")` |
| `where_not_eq(col, val)` | `col != ?` | `.where_not_eq("role", "admin")` |
| `where_like(col, pat)` | `col LIKE ?` | `.where_like("name", "%alice%")` |
| `where_gt(col, val)` | `col > ?` | `.where_gt("age", "18")` |
| `where_lt(col, val)` | `col < ?` | `.where_lt("price", "100")` |
| `where_in(col, vals)` | `col IN (?, ?, ...)` | `.where_in("id", &["1", "2", "3"])` |
| `where_null(col)` | `col IS NULL` | `.where_null("deleted_at")` |
| `where_not_null(col)` | `col IS NOT NULL` | `.where_not_null("email")` |

### 4. Pagination with Pageable and Page\<T\>

`Pageable` is extracted from query params:

```rust
use r2e::prelude::*; // Query, Pageable, Page

#[get("/data/users")]
async fn list(
    &self,
    Query(pageable): Query<Pageable>,
) -> Result<Json<Page<UserEntity>>, HttpError> {
    let qb = QueryBuilder::new("users")
        .order_by("id", true)
        .limit(pageable.size)
        .offset(pageable.offset());

    let (sql, _params) = qb.build_select("id, name, email");
    let rows: Vec<UserEntity> = sqlx::query_as(&sql)
        .fetch_all(&self.pool)
        .await?;

    let (count_sql, _) = QueryBuilder::new("users").build_count();
    let total: (i64,) = sqlx::query_as(&count_sql)
        .fetch_one(&self.pool)
        .await?;

    Ok(axum::Json(Page::new(rows, &pageable, total.0 as u64)))
}
```

### Pageable parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `page` | `u64` | `0` | Page number (starts at 0) |
| `size` | `u64` | `20` | Number of elements per page |
| `sort` | `Option<String>` | `None` | Sort field (optional) |

### HTTP request example

```bash
curl "http://localhost:3000/data/users?page=0&size=10"
```

### Page\<T\> response

```json
{
    "content": [
        {"id": 1, "name": "Alice", "email": "alice@example.com"},
        {"id": 2, "name": "Bob", "email": "bob@example.com"}
    ],
    "page": 0,
    "size": 10,
    "total_elements": 3,
    "total_pages": 1
}
```

### 5. Search with QueryBuilder

```rust
#[derive(serde::Deserialize)]
struct SearchParams {
    name: Option<String>,
    email: Option<String>,
}

#[get("/data/users/search")]
async fn search(
    &self,
    Query(params): Query<SearchParams>,
) -> Result<axum::Json<Vec<UserEntity>>, r2e_core::HttpError> {
    let mut qb = QueryBuilder::new("users");

    if let Some(ref name) = params.name {
        qb = qb.where_like("name", &format!("%{name}%"));
    }
    if let Some(ref email) = params.email {
        qb = qb.where_eq("email", email);
    }

    let (sql, params_vec) = qb.order_by("id", true).build_select("id, name, email");

    let mut query = sqlx::query_as::<_, UserEntity>(&sql);
    for p in &params_vec {
        query = query.bind(p);
    }

    let rows = query.fetch_all(&self.pool).await?;
    Ok(axum::Json(rows))
}
```

## Validation criteria

```bash
# Paginated list
curl "http://localhost:3000/data/users?page=0&size=2"
# → {"content":[...],"page":0,"size":2,"total_elements":3,"total_pages":2}

# Search by name
curl "http://localhost:3000/data/users/search?name=Alice"
# → [{"id":1,"name":"Alice","email":"alice@example.com"}]

# Search by exact email
curl "http://localhost:3000/data/users/search?email=bob@example.com"
# → [{"id":2,"name":"Bob","email":"bob@example.com"}]
```

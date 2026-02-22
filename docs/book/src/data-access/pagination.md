# Pagination

R2E provides `Pageable` for extracting pagination parameters from query strings and `Page<T>` for structured paginated responses.

## `Pageable` extractor

`Pageable` is an Axum `Query` extractor with pagination parameters:

```rust
use r2e::r2e_data::{Pageable, Page};

#[get("/")]
async fn list(&self, Query(pageable): Query<Pageable>) -> Result<Json<Page<User>>, HttpError> {
    // pageable.page  — page number (0-based, default: 0)
    // pageable.size  — items per page (default: 20)
    // pageable.sort  — sort field (optional)
    // ...
}
```

### Query string format

```
GET /users?page=0&size=20&sort=name
GET /users?page=2&size=10&sort=created_at,desc
```

## `Page<T>` response

`Page<T>` wraps paginated results with metadata:

```rust
#[derive(Serialize)]
pub struct Page<T> {
    pub content: Vec<T>,
    pub total_elements: i64,
    pub total_pages: i64,
    pub page: i64,
    pub size: i64,
}
```

### Constructing a `Page`

```rust
Page::new(items, &pageable, total_count)
```

## Complete example

```rust
#[get("/")]
async fn list(
    &self,
    Query(pageable): Query<Pageable>,
) -> Result<Json<Page<User>>, HttpError> {
    let offset = pageable.page * pageable.size;

    // Get total count
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?;

    // Get page of results
    let users = sqlx::query_as::<_, User>(
        "SELECT id, name, email FROM users ORDER BY id LIMIT ? OFFSET ?"
    )
    .bind(pageable.size)
    .bind(offset)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| HttpError::Internal(e.to_string()))?;

    Ok(Json(Page::new(users, &pageable, total.0)))
}
```

### Response format

```json
{
  "content": [
    {"id": 1, "name": "Alice", "email": "alice@example.com"},
    {"id": 2, "name": "Bob", "email": "bob@example.com"}
  ],
  "total_elements": 42,
  "total_pages": 3,
  "page": 0,
  "size": 20
}
```

## With QueryBuilder

```rust
use r2e::r2e_data::QueryBuilder;

#[get("/")]
async fn list(
    &self,
    Query(pageable): Query<Pageable>,
) -> Result<Json<Page<User>>, HttpError> {
    let mut qb = QueryBuilder::new("users");

    if let Some(ref sort) = pageable.sort {
        qb = qb.order_by(sort, true);
    }

    qb = qb.limit(pageable.size as usize)
            .offset((pageable.page * pageable.size) as usize);

    let (sql, params) = qb.build_select("id, name, email");
    let (count_sql, count_params) = QueryBuilder::new("users").build_count();

    // Execute both queries...
    // Return Page::new(users, &pageable, total)
}
```

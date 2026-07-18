# Pagination

R2E provides `Pageable` for extracting pagination parameters from query strings and `Page<T>` for structured paginated responses.

## `Pageable` extractor

`Pageable` is an Axum `Query` extractor with pagination parameters:

```rust
use r2e::prelude::{Pageable, Page};

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
    pub page: u64,
    pub size: u64,
    pub total_elements: u64,
    pub total_pages: u64,
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
    let offset = pageable.offset();

    // Get total count
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&self.pool)
        .await
        .map_err(|e| HttpError::internal(e.to_string()))?;

    // Get page of results
    let users = sqlx::query_as::<_, User>(
        "SELECT id, name, email FROM users ORDER BY id LIMIT ? OFFSET ?"
    )
    .bind(pageable.size as i64)
    .bind(offset as i64)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| HttpError::internal(e.to_string()))?;

    Ok(Json(Page::new(users, &pageable, total.0 as u64)))
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

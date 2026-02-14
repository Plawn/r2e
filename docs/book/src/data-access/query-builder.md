# QueryBuilder

R2E's `QueryBuilder` provides a fluent API for constructing SQL queries.

## Basic usage

```rust
use r2e::r2e_data::QueryBuilder;

let (sql, params) = QueryBuilder::new("users")
    .where_eq("email", "alice@example.com")
    .build_select("id, name, email");
// sql: "SELECT id, name, email FROM users WHERE email = ?"
// params: ["alice@example.com"]
```

## Where clauses

| Method | SQL | Example |
|--------|-----|---------|
| `where_eq(col, val)` | `col = ?` | `where_eq("status", "active")` |
| `where_like(col, val)` | `col LIKE ?` | `where_like("name", "%ali%")` |
| `where_gt(col, val)` | `col > ?` | `where_gt("age", "18")` |
| `where_lt(col, val)` | `col < ?` | `where_lt("price", "100")` |
| `where_gte(col, val)` | `col >= ?` | `where_gte("score", "50")` |
| `where_lte(col, val)` | `col <= ?` | `where_lte("count", "10")` |
| `where_in(col, vals)` | `col IN (?, ?)` | `where_in("id", &["1", "2"])` |
| `where_null(col)` | `col IS NULL` | `where_null("deleted_at")` |
| `where_not_null(col)` | `col IS NOT NULL` | `where_not_null("email")` |

## Ordering, limits, offsets

```rust
let (sql, params) = QueryBuilder::new("users")
    .where_like("name", "%ali%")
    .order_by("name", true)     // true = ASC, false = DESC
    .limit(10)
    .offset(20)
    .build_select("id, name, email");
// SELECT id, name, email FROM users WHERE name LIKE ? ORDER BY name ASC LIMIT 10 OFFSET 20
```

## Count queries

```rust
let (sql, params) = QueryBuilder::new("users")
    .where_eq("status", "active")
    .build_count();
// SELECT COUNT(*) FROM users WHERE status = ?
```

## Combining conditions

All where clauses are combined with AND:

```rust
let (sql, params) = QueryBuilder::new("users")
    .where_eq("status", "active")
    .where_like("name", "%ali%")
    .where_gt("age", "18")
    .order_by("created_at", false)
    .build_select("*");
// SELECT * FROM users WHERE status = ? AND name LIKE ? AND age > ? ORDER BY created_at DESC
```

## Using with SQLx

```rust
let (sql, params) = QueryBuilder::new("users")
    .where_eq("status", "active")
    .limit(10)
    .build_select("id, name, email");

let mut query = sqlx::query_as::<_, User>(&sql);
for param in &params {
    query = query.bind(param);
}
let users = query.fetch_all(&self.pool).await?;
```

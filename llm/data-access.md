---
topic: data-access
features: data
tokens: ~400
requires: managed-resources
---

## Data Access

### TL;DR

- Requires feature `data` (+ `data-sqlx`/`sqlite`/`postgres`/`mysql`); the trait lives in `r2e_data`.
- Implement `Entity` for each row type: `type Id`, `table_name()`, `id_column()`, `columns()`, `id()`.
- `Repository<T, ID>` is the async CRUD trait; `SqlxRepository` is the implementation you get.
- Build ad-hoc SQL with `QueryBuilder` (`where_eq`, `where_like`, `order_by`, `limit`, `offset`).
- Paginate with the `Query<Pageable>` query-string extractor and answer `Page::new(entities, &pageable, total)`.
- Transactions and pools are not part of this API — see llm/managed-resources.md.

Requires feature: `data` (+ `data-sqlx`/`sqlite`/`postgres`/`mysql`)

```rust,ignore
use r2e_data::Entity;

impl Entity for UserEntity {
    type Id = i64;
    fn table_name() -> &'static str { "users" }
    fn id_column() -> &'static str { "id" }
    fn columns() -> &'static [&'static str] { &["id", "name", "email"] }
    fn id(&self) -> &i64 { &self.id }
}
```

- `Repository<T, ID>` — async CRUD trait; `SqlxRepository` implements it.
- `QueryBuilder` — fluent SQL (`where_eq`, `where_like`, `order_by`, `limit`, `offset`).
- `Pageable` (query-string extractor) + `Page<T>` (response wrapper):

```rust
#[controller(path = "/users")]
pub struct UserController;

#[routes]
impl UserController {
    #[get("/")]
    async fn list_paged(&self, Query(pageable): Query<Pageable>) -> JsonResult<Page<UserEntity>> {
        Ok(Json(Page::new(entities, &pageable, total)))
    }
}
# fn main() {}
```

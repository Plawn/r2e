---
topic: validation
features: core
tokens: ~1500
requires: core-concepts
---

## Validation

### TL;DR

- Validation is always available (`garde`), no feature flag; `Json<T>` auto-validates when `T: garde::Validate` and rejects with 400 + field errors.
- Aggregate path/query/header inputs with `#[derive(Params)]` and take the struct directly as a handler parameter — all its fields appear in the OpenAPI spec.
- Annotate fields with `#[param(path)]`, `#[query]`, `#[header("X-...")]`; a field with none of them is a query parameter named after the field.
- There is no `#[params(...)]` renaming spelling: `#[serde(rename)]`, `rename_all`, `default`, `skip`, `flatten` are read as-is.
- Precedence: explicit R2E name > `#[serde(rename)]` > `#[serde(rename_all)]` > field identifier; `#[param(default)]` > `#[serde(default)]`.
- `#[serde(skip)]` combined with an R2E param attribute is a compile error.
- Migrate `Query<T>` to `Params` by adding the derive and dropping the wrapper; the only visible change is the 400 body, controlled app-wide by `server.params-rejection-format` (`json` default, `plain-text` for compatibility).
- To handle a rejection yourself, take `Result<Query<T>, QueryRejection>` — `QueryRejection`/`PathRejection`/`FormRejection`/`JsonRejection` come from `r2e::http`.

Always available (via `garde`). `Json<T>` extraction auto-validates when
`T: garde::Validate`; failures return 400 with field errors.

```rust
use garde::Validate;
use schemars::JsonSchema;

#[derive(Deserialize, Validate, JsonSchema)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,
    #[garde(email)]
    pub email: String,
}
```

### `#[derive(Params)]` — aggregated path/query/header params

```rust
use garde::Validate;

#[derive(Params, Validate)]
struct GetUserParams {
    #[param(path)]
    #[garde(skip)]
    id: u64,
    #[query]
    #[param(default = 1u32)]
    #[garde(range(min = 1))]
    page: u32,
    #[header("X-Tenant-Id")]
    #[garde(length(min = 1))]
    tenant_id: String,
}

#[controller(path = "/users")]
pub struct UserController;

#[routes]
impl UserController {
    #[get("/{id}")]
    async fn get(&self, params: GetUserParams) -> Json<User> { Json(user_by_id(params.id)) }
}
# fn main() {}
```

All fields appear automatically in the OpenAPI spec. Two spellings the
compiler insists on: `#[derive(Validate)]` wants every field annotated, so a
field with no rule needs `#[garde(skip)]`, and `#[param(default = …)]` goes
through `Into`, so write a typed literal (`1u32`, not `1`).

A field with **no** `#[param]`/`#[query]`/`#[header]` attribute is a query
parameter named after the field. That makes the derive a drop-in for
`Query<T>`.

**Serde attributes are read, not duplicated.** There is no `#[params(...)]`
renaming spelling: the derive honours the `#[serde(...)]` attributes a struct
already carries, so a shipped `Query<T>` payload migrates untouched.

| Attribute | Effect on the wire name / value |
|---|---|
| `#[serde(rename_all = "camelCase")]` (struct) | Renames every field. All serde cases: `lowercase`, `UPPERCASE`, `PascalCase`, `camelCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`, `SCREAMING-KEBAB-CASE` |
| `#[serde(rename = "q")]` (field) | Exact wire name (also `rename(deserialize = "…")`) |
| `#[serde(default)]` / `#[serde(default = "path")]` | Missing value falls back to `Default` / that function |
| `#[serde(skip)]` / `#[serde(skip_deserializing)]` | Field is never read from the request; built with `Default` |
| `#[serde(flatten)]` (field) | The nested `Params` struct's own keys are read from the same request (same as a bare `#[params]`) |

Precedence: an explicit R2E name (`#[query("q")]`, `#[param(path, name = "…")]`,
`#[header("X-…")]`) wins over `#[serde(rename)]`, which wins over
`#[serde(rename_all)]`, which wins over the field identifier. Likewise
`#[param(default = …)]` wins over `#[serde(default)]`. `#[serde(skip)]`
combined with an R2E param attribute is a compile error.

**Migrating `Query<T>` → `Params`.** Add `Params` to the existing derive list
and drop the `Query` wrapper in the handler; keep the `Deserialize` derive if
anything else still deserializes the type. Nothing else changes — the serde
renames keep producing the same wire names, and every field is now visible in
the OpenAPI spec:

```rust
// before
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    #[serde(rename = "q")]
    query: String,
    page_size: Option<u32>,
}

#[controller(path = "/search")]
pub struct SearchController;

#[routes]
impl SearchController {
    #[get("/")]
    async fn search(&self, Query(q): Query<SearchQuery>) -> Json<Hits> {
        Json(search_hits(&q.query, q.page_size))
    }
}
# fn main() {}
```

```rust
// after — same `?q=…&pageSize=…` contract
#[derive(Deserialize, Params)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    #[serde(rename = "q")]
    query: String,
    page_size: Option<u32>,
}

#[controller(path = "/search")]
pub struct SearchController;

#[routes]
impl SearchController {
    #[get("/")]
    async fn search(&self, q: SearchQuery) -> Json<Hits> {
        Json(search_hits(&q.query, q.page_size))
    }
}
# fn main() {}
```

The one visible difference is the 400 body. `Query<T>` answers with plain text
from serde; `Params` answers with a JSON problem body by default. It is an
**app-level** setting, never a per-struct one, read once at `build_state()`:

```yaml
server:
  params-rejection-format: json        # default; { "error": "...", "message": "..." }
  # params-rejection-format: plain-text  # raw `Query<T>` compatibility
```

Unknown values fail boot. In Rust the enum is `r2e::ParamsRejectionFormat`
(`Json` | `PlainText`); the JSON body is `{"error": "<message>"}`.

Axum's own rejection types are re-exported for handlers that take a
`Result<Query<T>, QueryRejection>`: `QueryRejection`, `PathRejection`,
`FormRejection`, `JsonRejection` from `r2e::http` (and `r2e::http::rejection`).

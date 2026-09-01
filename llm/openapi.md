---
topic: openapi
features: openapi
tokens: ~700
requires: core-concepts
---

## OpenAPI

### TL;DR

- Requires feature `openapi`; add `schemars = "1"` and derive `JsonSchema` on request/response types.
- Install `.plugin(OpenApiPlugin::new(OpenApiConfig::new("API", "1.0.0")))` — install order is irrelevant; `.with_docs_ui(true)` serves `/docs`, the spec is at `/openapi.json`.
- Schemas are auto-detected from `Json<T>` parameters and return types; override with `#[status(N)]` / `#[returns(T)]`, and doc comments become summary/description.
- A body that cannot be mapped (an `impl Trait` or non-`Json` return, a type without `JsonSchema`) is documented without a body and warned about once at boot; read them programmatically with `r2e::r2e_openapi::spec_warnings(&routes)`.
- The tag defaults to the controller struct name; `#[controller(tag = "...")]` merges several controllers under one tag.

Requires feature: `openapi`. Generates OpenAPI 3.1.0; users add `schemars = "1"`
and derive `JsonSchema` on request/response types.

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(OpenApiPlugin::new(
    OpenApiConfig::new("My API", "1.0.0")
        .with_description("API description")
        .with_docs_ui(true),               // serves /docs
))   // install order irrelevant — the spec is built from a Routes-stage effect
# }
```

Spec at `/openapi.json`. Request/response schemas auto-detected from `Json<T>`
params and return types; `#[status(N)]` / `#[returns(T)]` override; doc
comments become summary/description; 401/403 only emitted on authed routes.

When a route's successful response body can't be mapped to a schema (an
`impl Trait` return, or a concrete non-`Json` type), the spec still generates
but the response is documented **without a body**. Instead of dropping this
silently, spec generation logs a `tracing::warn!` **once at boot** naming the
method, path, and offending return type, and suggesting `#[returns(T)]` /
`Json<T>`. Named bodies (request or response) whose type lacks
`schemars::JsonSchema` — rendered as a generic `object` — are warned about too.
The gaps are also available programmatically via
`r2e::r2e_openapi::spec_warnings(&routes) -> Vec<SpecWarning>` (each carries
`method`, `path`, a `SchemaGap`, and a `.message()`).

**Tags.** A route's OpenAPI tag defaults to the controller's struct name.
`#[controller(path = "…", tag = "…")]` overrides it, so several controllers can
publish under one tag:

```rust
#[controller(path = "/catalog/items", tag = "Catalog")]
struct CatalogItemsController;

#[controller(path = "/catalog/categories", tag = "Catalog")]
struct CatalogCategoriesController;   // both merge under the "Catalog" tag
# fn main() {}
```

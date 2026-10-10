---
topic: openapi
features: openapi
tokens: ~2000
requires: core-concepts
---

## OpenAPI

### TL;DR

- Requires feature `openapi`; add `schemars = "1"` and derive `JsonSchema` on request/response types.
- Install `.plugin(OpenApiPlugin::new(OpenApiConfig::new("API", "1.0.0")))` — install order is irrelevant; `.with_docs_ui(true)` serves `/docs`, the spec is at `/openapi.json`.
- Bodies are documented through two traits the framework implements for its own types: the body parameter's `RequestBodySchema` (`Json<T>`, `Form<T>`, `Bytes`, `String`, `Multipart`, `TypedMultipart<T>`) and the return type's `ResponseBodySchema` (`Json<T>`, `String`, `Html<T>`, `Bytes`, `Vec<u8>`, `()`, `StatusCode`, `Redirect`, `Sse<S>`; `Result<T, E>` and `(StatusCode, T)` tuples delegate to `T`). Custom extractors / response types implement the trait; `#[status(N)]` / `#[returns(T)]` override; doc comments become summary/description.
- A body-position parameter or a return type without the trait (an `impl Trait` return, a `Json<T>` whose `T` lacks `JsonSchema`) is documented without a body and warned about once at boot; read the warnings programmatically with `r2e::r2e_openapi::spec_warnings(&routes)`.
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

Spec at `/openapi.json`. Request and response bodies are documented through
`RequestBodySchema` / `ResponseBodySchema` (below) — the macro never matches
type names; `#[status(N)]` / `#[returns(T)]` override; doc comments become
summary/description.

**Error responses** come from what the route can actually fail with. The
`#[routes]` macro records the route's `RejectionKind`s in
`RouteInfo::rejection_kinds` (JSON body → 415/413/400/422, `Form<T>` body →
415/413/400/422, GET `Form<T>` → 400, `Path` → 400, `Query`/`#[derive(Params)]`
→ 400, garde `Validate` parameter → 400, identity — required or `Option<..>` —
→ 401, `#[roles]`/guards → 403, a rate-limit guard → 429, always 500; SSE and
WS routes are inferred from their parameters the same way)
and the builder emits one response per distinct status the route's **error
envelope** maps them to (`ErrorSchema::status_of`), with the envelope's
`body_schema` / `body_schema_for(kind)` as the component (+ its
`extra_statuses`). The envelope is the handler's `Result<T, E>` error type when
it is one, else the application's (`AppBuilder::error_projection::<E>()`, read
from the `ErrorProjector` bean by the plugin; `OpenApiConfig::with_error_schema::<E>()`
for direct `build_spec` callers), else `HttpError` (`ErrorResponse` /
`ValidationErrorResponse`). Runtime and spec call the same `status_of`, so an
envelope remapping 422 → 400 documents 400 only. Bodies are deduplicated by schema (the same body under two names is
documented once, under the first-recorded name — the route envelope's); distinct
bodies on one status render as `anyOf` (a validation body is also a valid plain
error body, so `oneOf` would reject it). An error body whose component name is
already taken by a different schema (a DTO, a registry entry, a nested `$defs`
type, another envelope's body) leaves that component alone and is documented
**inline**, with a boot warning (`SchemaGap::ErrorBodyInlined`). Every route also documents the panic 500 with the
**application** envelope's body (the catch-panic layer renders through the
`ErrorProjector`, never the route's envelope) — an `anyOf` beside the route's
own 500 body when they differ. Nested types in an envelope body schema are
promoted to `components/schemas` like any other schema.

**Request body.** The handler's last parameter, when it reads the body
(`FromRequest`), is documented through `r2e::di::meta::RequestBodySchema`
(`content_type()`, `body_schema()` — defaults to `None`, `rejection_kinds()`).
Framework impls: `Json<T>` (`application/json`, schema from `T: JsonSchema`),
`Form<T>` (`application/x-www-form-urlencoded`), `Bytes`
(`application/octet-stream`), `String` (`text/plain`), `Multipart` /
`TypedMultipart<T>` (`multipart/form-data`, the typed one with
`#[derive(FromMultipart)]`'s schema). `Option<..>` around the extractor makes
the body optional (`required: false`). A custom extractor implements the trait;
a body-position type without it has no request body in the spec and is warned
about at boot (`SchemaGap::MissingRequestBody`).

**Response body.** The return type is probed for
`r2e::di::meta::ResponseBodySchema`, through `Result<T, E>` (→ `T`, so every
alias such as `ApiResult<T>` works) and the `(StatusCode, T)` /
`(HeaderMap, T)` / `(StatusCode, HeaderMap, T)` tuples. Framework impls:
`Json<T>` (`application/json`, schema from `T: JsonSchema`), `String` /
`&'static str` / `Cow<'static, str>` (`text/plain`), `Html<T>` (`text/html`),
`Bytes` / `Vec<u8>` (`application/octet-stream`), `()` / `StatusCode` /
`Redirect` (no body), `Sse<S>` (`text/event-stream`). A custom type implements
the trait: `response_contents()` lists every media type it can be served as
(`ResponseContent::json(..)`, `::event_stream(..)`, `::text()`, `::html()`,
`::binary()`, `::new(ct, ..)`), each with an optional `(component name,
schema)` — `r2e::di::meta::schema_of::<T>()` (re-exported as
`r2e_openapi::schema_of`) builds one from a `JsonSchema` type. A handler
answering JSON or an SSE stream depending on the request returns an enum and
documents both under its success status:

```rust,ignore
enum ChatReply {
    Json(Json<Completion>),
    Stream(Sse<BoxStream<'static, Result<SseEvent, Infallible>>>),
}

impl ResponseBodySchema for ChatReply {
    fn response_contents() -> Vec<ResponseContent> {
        vec![
            ResponseContent::json(Some(schema_of::<Completion>())),
            ResponseContent::event_stream(Some(schema_of::<Chunk>())),
        ]
    }
}
```

A `None` schema renders from the media type: `text/*` → `{"type": "string"}`,
`application/octet-stream` → binary string, form media types → free-form
object, anything else → `{}`. Return types containing `impl Trait` are never
probed: declare the body with `#[returns(T)]`, where `T` is probed for
`ResponseBodySchema` first and as `Json<T>` second (so a `JsonSchema` DTO
works directly).

When a route's successful response body can't be mapped (an `impl Trait`
return, or a concrete type without `ResponseBodySchema` — a `Json<T>` whose
`T` lacks `JsonSchema` included), the spec still generates but the response is
documented **without a body**. Instead of dropping this silently, spec
generation logs a `tracing::warn!` **once at boot** naming the method, path,
and offending type, and suggesting `#[returns(T)]` / the trait
(`SchemaGap::MissingResponseBody`; `SchemaGap::MissingRequestBody` is the
request-side twin). The gaps are also available programmatically via
`r2e::r2e_openapi::spec_warnings(&routes) -> Vec<SpecWarning>` (each carries
`method`, `path`, a `SchemaGap`, and a `.message()`);
`r2e::r2e_openapi::build_spec_with_warnings(&config, &routes)` returns the spec
plus every warning, inlined error bodies included.

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

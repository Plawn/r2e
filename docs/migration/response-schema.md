# Migration: body schemas through traits (0.5 → 0.6)

0.6 makes `RequestBodySchema` / `ResponseBodySchema` the **only** way a request
or response body reaches the OpenAPI spec. The `#[routes]` macro no longer
recognises `Json<T>`, `JsonResult<T>`, `StatusResult`, `Bytes`, `String`,
`Multipart`, … by their *names*: it probes the body parameter's type and the
return type for the two traits, and R2E ships the impls for its own types. A
custom extractor or response type is documented exactly like a framework one —
implement the trait — and type aliases (`ApiResult<T>`, your own
`Result<T, E>` alias) work because `Result<T, E>` delegates to `T`.

Handlers that use the framework's types need no change. The source-level
breaks are in `RouteInfo` (anyone building or reading route metadata by hand),
in `SchemaGap` (anyone matching spec warnings), in component naming, and in
`#[returns(T)]`'s meaning.

## What changed

### Request bodies

The handler's last parameter, when it reads the body (`FromRequest`), is
documented when its type implements `r2e::di::meta::RequestBodySchema`:

| Extractor | Media type | Schema | Rejection kinds |
|---|---|---|---|
| `Json<T>` | `application/json` | `T`'s (`T: JsonSchema`, needs feature `openapi`) | 415 / 413 / 400 / 422 |
| `Form<T>` | `application/x-www-form-urlencoded` | free-form object | 415 / 413 / 400 / 422 |
| `Bytes` | `application/octet-stream` | binary string | 413 / 400 |
| `String` | `text/plain` | string | 413 / 400 |
| `Multipart` | `multipart/form-data` | free-form object | 415 / 413 / 400 |
| `TypedMultipart<T>` | `multipart/form-data` | `T`'s (`#[derive(FromMultipart)]`) | 415 / 413 / 400 / 422 |

`Option<..>` around any of them → `required: false`. Any other body-position
type without the trait is **undocumented** and flagged once at boot
(`SchemaGap::MissingRequestBody`) — in 0.5 a `Json<T>` whose `T` lacked
`JsonSchema` was documented as a generic `object`; it now needs the derive (or
is skipped, with the warning). `RequestBodySchema::body_schema()` now defaults
to `None`, so a schemaless extractor only declares `content_type()` and
`rejection_kinds()`.

### Responses

The return type is probed for `r2e::di::meta::ResponseBodySchema`, through
`Result<T, E>` (→ `T`) and the `(StatusCode, T)` / `(HeaderMap, T)` /
`(StatusCode, HeaderMap, T)` tuples:

| Return type | Documented as |
|---|---|
| `Json<T>` | `application/json`, `T`'s schema (`T: JsonSchema`) |
| `String`, `&'static str`, `Cow<'static, str>` | `text/plain`, string |
| `Html<T>` | `text/html`, string |
| `Bytes`, `Vec<u8>` | `application/octet-stream`, binary string |
| `()`, `StatusCode`, `Redirect` | no body |
| `Sse<S>` | `text/event-stream` |
| `impl Trait` | unmapped → use `#[returns(T)]` |

A concrete type without the trait stays unmapped (`SchemaGap::MissingResponseBody`,
same warning as 0.5). In 0.5 `String` / `StatusCode` / `StatusResult` were
"no schema" by name; they now carry their media type (or no body) through
their impl, so a `String` handler documents a `text/plain` string body.

### `#[returns(T)]`

`T` is no longer required to be a `JsonSchema` DTO. It is probed for
`ResponseBodySchema` first, then as `Json<T>`: `#[returns(Widget)]` with
`Widget: JsonSchema` documents `application/json` as before, and
`#[returns(ChatReply)]` with `ChatReply: ResponseBodySchema` documents every
media type the type lists. A `T` that is neither is unmapped and warned about.

### Component names

Components are named by schemars (`T::schema_name()`), not by a macro-side
rendering of the Rust type. Generic DTOs change name:

| 0.5 | 0.6 |
|---|---|
| `Vec_User` | `Array_of_User` |
| `Option_User` | `Nullable_User` |
| `Page_User` | `Page_User` (unchanged — schemars keeps the Rust name for user types) |

Clients generated from the spec, or tests pinning `components/schemas` keys,
must follow.

## Breaking changes

| 0.5 | 0.6 | Action |
|---|---|---|
| `RouteInfo { request_body_type, request_body_schema, request_body_content_type, request_body_required, .. }` | `RouteInfo { request_body: Option<RequestBody { content_type, schema: Option<(String, Value)>, required }>, request_body_unmapped: Option<String>, .. }` | Struct literals: replace the four fields with `request_body: None, request_body_unmapped: None`; readers go through `route.request_body.as_ref()`. |
| `RouteInfo { response_type, response_schema, .. }` | `RouteInfo { response_contents: Vec<ResponseContent>, .. }` (`response_unmapped` kept) | Literals: `response_contents: Vec::new()`; a JSON response is `vec![ResponseContent::json(Some((name, schema)))]`. |
| `SchemaGap::{MissingResponseBody, SchemalessResponseBody, SchemalessRequestBody, ErrorBodyInlined}` | `SchemaGap::{MissingRequestBody { type_name }, MissingResponseBody { type_name }, ErrorBodyInlined { component }}` | Exhaustive matches: drop the two `Schemaless*` arms (a schemaless body is now rendered from its media type, not warned about), add `MissingRequestBody`. |
| `r2e_openapi::schema_of::<T>()` | `r2e_core::di::meta::schema_of::<T>()` (feature `openapi` on `r2e-core`; still re-exported by `r2e_openapi`) | Nothing for `r2e` facade users. Direct `r2e-core` users who implement `ResponseBodySchema` with schemas enable `features = ["openapi"]`. |
| `MultipartSchema { multipart_schema() }` | `MultipartSchema { multipart_schema(), schema_name() }` | Hand-written impls add `fn schema_name() -> &'static str`; `#[derive(FromMultipart)]` emits it. |
| `RequestBodySchema::body_schema()` required | defaults to `None` | Nothing; schemaless impls may drop the method. |
| `#[returns(T)]` requires `T: JsonSchema` | `T: ResponseBodySchema` or `T: JsonSchema` | Nothing for existing uses. |
| `Json<T>` body / return with `T: !JsonSchema` → generic `object` + warning | undocumented + warning | Derive `JsonSchema` on the DTO. |
| `Vec_User`-style component names | schemars names (`Array_of_User`) | Update generated clients / pinned spec tests. |

## Custom types

```rust,ignore
use r2e::di::meta::{RequestBodySchema, ResponseBodySchema, ResponseContent, schema_of};
use r2e::RejectionKind;

// A CSV body extractor (last parameter, `FromRequest`).
impl RequestBodySchema for CsvBody {
    fn content_type() -> &'static str { "text/csv" }
    fn rejection_kinds() -> Vec<RejectionKind> {
        vec![RejectionKind::UnsupportedMediaType, RejectionKind::MalformedBody]
    }
}

// A reply served as JSON or as an SSE stream.
impl ResponseBodySchema for ChatReply {
    fn response_contents() -> Vec<ResponseContent> {
        vec![
            ResponseContent::json(Some(schema_of::<Completion>())),
            ResponseContent::event_stream(Some(schema_of::<Chunk>())),
        ]
    }
}
```

`ResponseContent::{json, event_stream, text, html, binary, new}` cover the
usual media types; a `None` schema renders from the media type (`text/*` →
string, `application/octet-stream` → binary string, form media types → object,
anything else → `{}`).

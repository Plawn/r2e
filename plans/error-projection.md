# Error projection: one semantic failure, one rendering point per route

Ticket: #1072 (typed error per controller). This plan generalises the ticket:
instead of bolting `From<Rejection>` onto one more exit door, it gives the
framework a **single, typed failure value** and a **single projection point per
route**, so every way a request can fail (bad body, bad auth, guard, validation,
managed resource, handler error) reaches the wire through the same envelope and
is documented from the same source of truth.

Status: **approved design, 2026-10-09**. Decisions in §10 are locked. Nothing
implemented yet; branch `feat/1072-error-projection`.

---

## 1. The problem, generalised

A request can fail at eight places. Today each one renders itself, and only one
of them is under the application's control:

| # | Exit door | Produced by | Type at the exit | Rendered by | Typed for the app? | In the spec? |
|---|---|---|---|---|---|---|
| 1 | handler param rejection (`Json`, `Path`, `Query`, `Form`, `Params`, custom) | axum `Handler` impl, before any R2E code | `T::Rejection` | the extractor's own `IntoResponse` (axum: plain text; `Json`: text; `Params`: global `ParamsRejectionFormat`) | no | hardcoded `ValidationErrorResponse` on 400 |
| 2 | `#[inject(request)]` / struct-level identity | `__R2eRequestData::from_request_parts` | collapsed to `Response` (`controller_codegen.rs:349`) | the field type's `IntoResponse` | no | hardcoded `ErrorResponse` on 401/403 |
| 3 | guard | `Guard::check` | `Result<(), Response>` | `GuardError::into_response` → `{"error": msg}` | no | same |
| 4 | garde validation | `__maybe_validate` | `Box<Response>` | `convert_garde_report` → `{"error":"Validation failed","details":[…]}` | no | `ValidationErrorResponse` |
| 5 | managed acquire / finalize | `ManagedResource` | `Error: Into<Response>` | user's `ManagedErr<E>` | no | nothing |
| 6 | handler return | the route body | `Result<T, E>` | `E: IntoResponse` | **yes** | `#[returns]` only covers the success body |
| 7 | panic | catch-panic layer | — | `static_json(500, {"error":"Internal server error"})` | no | hardcoded `ErrorResponse` on 500 |
| 8 | no route / wrong method / body limit | axum router + layers | — | axum plain-text 404 / 405 / 413 | no | nothing |

Three consequences, all seen in llm-engine-rust:

- An app that must answer **every** failure in a foreign envelope (OpenAI,
  Anthropic, RFC 9457) has to write never-failing extractors and unwrap them by
  hand in every handler.
- The OpenAPI builder documents a shape the app does not emit.
- Each transport re-derives the meaning of a failure from its rendering: MCP
  folds a guard `Response` back into an `McpError` by reading its status **and
  body** (`r2e-mcp/src/guard.rs:78`), gRPC has a separate guard trait returning
  `tonic::Status`.

The root cause is **early erasure**: the framework turns a failure into a
`Response` at the first opportunity, so nothing downstream can re-interpret it.

## 2. Principle

> Separate the **fault** (what went wrong, a typed value) from the **envelope**
> (how it is written on the wire). Keep the fault typed until the last possible
> moment. Erase it exactly once per route, through the route's projector.

Four layers, connected only by std `From`/`Into` (plus one small trait for
static OpenAPI metadata, which has no value to convert):

| Layer | Type | To the next layer |
|---|---|---|
| **Fault** | `JsonRejection`, `PathRejection`, `ParamError`, `SecurityError`, `GuardError`, `RolesDenied`, `RateLimited`, `FgaDenied`, `TenantError`, garde report, `ManagedResource::Error`, `Response` (opaque) … | `impl From<X> for Rejection` |
| **Hub** | `Rejection` (kind, status, message, details, headers, source) | — |
| **Envelope** | the route's `E`; default `HttpError` | `E: From<Rejection> + IntoHttpResponse + ErrorSchema` |
| **Wire** | `Response` / `McpError` / `tonic::Status` | `IntoHttpResponse` / `From<Rejection> for McpError` / `From<Rejection> for tonic::Status` |

Concretely:

1. Every framework-side failure converts into one value, `Rejection`, carrying a
   closed `RejectionKind`, a status, a message, optional structured details,
   optional source, and the headers the failure semantically owns
   (`WWW-Authenticate`, `Retry-After`). `?` does the conversion in user code.
2. A route has one envelope type `E`. The generated handler converts every
   failure with `E::from(rejection)` and renders `E` once, at the end. `E` is
   also what the handler returns in `Err`.
3. Projection has two levels and no attribute: the route's **return type**
   `Result<T, E>` when `E: From<Rejection> + IntoHttpResponse + ErrorSchema`,
   else the app-level `error_projection::<E>()` (default `HttpError`, the
   JAX-RS `ExceptionMapper` equivalent). The default reproduces 0.4.0 byte for
   byte, so existing apps see no change. (Decision 2026-10-09: `#[error(E)]`
   and `#[routes(error = E)]` dropped — a route declared infallible that is
   not is a definition problem R2E does not fix for the developer.)
4. The spec is derived from the same data: the macro knows which
   `RejectionKind`s a route can produce (it sees the extractors, the identity,
   the guards, the validation); `E::status_of(kind)` is applied **before**
   `E::from` at runtime and read by the OpenAPI builder, so runtime and
   documentation cannot drift for a conforming projector.
5. Transports that are not HTTP (MCP, gRPC) map `Rejection` by kind, never by
   parsing a response.

## 3. Building blocks (r2e-core)

### 3.1 `Rejection` and `RejectionKind`

```rust
// r2e-core/src/error/rejection.rs
#[non_exhaustive]
pub struct Rejection {
    pub kind: RejectionKind,
    /// Starts at `kind.default_status()`; the generated code overwrites it with
    /// `E::status_of(kind)` before projection. `From<Rejection>` impls MUST
    /// read this field, never the table.
    pub status: StatusCode,
    pub message: Cow<'static, str>,
    pub details: Option<serde_json::Value>, // validation field errors, …
    pub headers: HeaderMap,                 // WWW-Authenticate, Retry-After, …
    pub source: Option<Arc<dyn Error + Send + Sync>>,
}

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RejectionKind {
    // request shape
    MissingContentType,   // 415
    UnsupportedMediaType, // 415
    PayloadTooLarge,      // 413
    BodyRead,             // 400 (status carried by the io failure)
    MalformedBody,        // 400 — syntax / eof
    InvalidBody,          // 422 — deserialized but semantically wrong (serde data error)
    InvalidPath,          // 400
    InvalidQuery,         // 400
    InvalidForm,          // 400
    InvalidHeader,        // 400
    BadRequest,           // 400 — a client fault with no closer kind (`GuardError`, `from_status` fallback)
    Validation,           // 400 — garde report in `details` (byte-equal with 0.4; `InvalidBody` is the 422)
    // auth
    Unauthenticated,      // 401
    Forbidden,            // 403
    // resource
    NotFound,             // 404
    Conflict,             // 409
    RateLimited,          // 429
    // server
    Internal,             // 500
    Unavailable,          // 503
    Timeout,              // 504
    // escape hatch: a pre-rendered response the framework could not type
    Opaque,
}
```

`RejectionKind::default_status()` is the one table; `RejectionKind::from_status`
is its inverse for faults that only carry a status (415/413/422/401/403/404/409/
429/503/504 map to their kind, any other 4xx to `BadRequest`, anything else to
`Internal`). `Opaque` carries the original `Response` in a private slot: a
projector passes it through when `ErrorSchema::opaque_passthrough()` (`HttpError`
does) or re-wraps it by status (an envelope projector does, body dropped).

`Rejection`'s `Display` is the bare client message (an envelope's `Display`
delegates to it); `Debug` has kind, status, details, headers and source.
`Rejection: Error` with `source()` wired to the carried cause.

Constructors keep the hot paths allocation-free: `Rejection::unauthenticated()`
is `Cow::Borrowed`, and the `HttpError` projector keeps today's pre-serialised
`static_json` bodies for the constant-message cases (`SecurityError` relies on
this for the unauthenticated-storm path; the plan preserves it).

### 3.2 Faults → hub: `From<X> for Rejection`

No framework trait. Each fault type gets a plain `From` impl in the crate that
owns it (r2e-core for the axum rejections, reached through
`r2e_http::extract::rejection`, so the axum boundary is untouched):

| Source | Kinds |
|---|---|
| `JsonRejection` | MissingContentType / BodyRead (+status) / MalformedBody / InvalidBody |
| `PathRejection`, `QueryRejection`, `FormRejection` (axum) | InvalidPath / InvalidQuery / InvalidForm |
| `MultipartRejection`, `MultipartError` (now `Error`) | MalformedBody / InvalidBody / PayloadTooLarge |
| `ParamError` (`#[derive(Params)]`) | InvalidQuery / InvalidHeader / InvalidPath from the new `ParamError::location: ParamLocation` |
| `HttpError` | by variant (`Validation` → Validation with details, `Custom` → by status, `WithSource` keeps source) |
| `SecurityError` | Unauthenticated / Unavailable, with `WWW-Authenticate: Bearer` |
| `GuardError` | kind `from_status`, the status is kept (a 418 stays 418) |
| `RolesDenied::{NoIdentity, Insufficient}` (r2e-security, new) | Forbidden, messages byte-equal to 0.4 |
| `FgaDenied::{NoIdentity, Denied, ObjectResolution(cause), CheckFailed}` (r2e-openfga, new) | Unauthenticated / Forbidden / BadRequest / Internal |
| `RateLimited { retry_after: Option<Duration> }` (r2e-rate-limit, new) | RateLimited, `Retry-After` = seconds rounded up when set |
| `ValidationErrorResponse` / garde report | Validation |
| `TenantError` | BadRequest / NotFound / Unavailable / Timeout / Internal; `TenantError::into_rejection(TenantStatuses)` keeps the configured statuses in `status`, `From` uses the defaults |
| `Response` | Opaque |
| `Infallible` | unreachable |

A foreign axum extractor whose rejection has no `From` impl still works: the
generated code probes `Into<Rejection>` with the same autoref trick used for
`JsonSchema` (`autoref_schema_probe`) and falls back to
`Rejection::from(r.into_response())` (Opaque). Nothing that compiles on 0.4
stops compiling.

### 3.3 Hub → envelope: `E: From<Rejection> + IntoHttpResponse + ErrorSchema`

```rust
/// Static documentation of an error envelope. Mandatory for every type used as
/// a projector (`#[error(E)]`, `#[routes(error = E)]`, `error_projection::<E>()`).
pub trait ErrorSchema {
    /// Status this envelope emits for `kind`. Applied to `Rejection::status`
    /// before `E::from`, and used by the OpenAPI builder. Default: the table.
    fn status_of(kind: RejectionKind) -> StatusCode { kind.default_status() }
    /// (component name, JSON Schema) of the body. `None` = undocumented body.
    fn body_schema() -> Option<(String, serde_json::Value)>;
    /// Per-kind body override when one envelope has several shapes
    /// (`HttpError` emits `ValidationErrorResponse` for `Validation`).
    fn body_schema_for(_kind: RejectionKind) -> Option<(String, serde_json::Value)> { None }
    /// Statuses the envelope can emit that no inferred kind covers
    /// (404 from a handler, 502 from a proxy, …), as `(status, description)`.
    fn extra_statuses() -> Vec<(StatusCode, &'static str)> { Vec::new() }
    /// Return an `Opaque` rejection untouched instead of re-wrapping it.
    /// `false` by default (one body shape); `HttpError` says `true` (0.4).
    fn opaque_passthrough() -> bool { false }
}
```

`Rejection::project::<E>()` is the one runtime helper behind every level: it
overwrites `status` with `E::status_of(kind)` **only when that differs from
`kind.default_status()`** (so a status the fault carried — a 413 body read, a
configured tenant status, a 418 `GuardError` — survives an envelope that does
not remap the kind), honours `opaque_passthrough`, calls
`E::from(self).into_http_response()`, then adds the hub `headers` with
`or_insert` (an envelope's own header of the same name wins).

- `HttpError: From<Rejection> + ErrorSchema` is the default projector and
  reproduces 0.4.0 exactly (`{"error": msg}`,
  `{"error":"Validation failed","details":[…]}`, Opaque passed through).
- `From<Rejection> for Response` goes through `HttpError` (legal: `Rejection` is
  local). It is what hand-written axum handlers merged via `merge_router` use.
- `#[derive(ApiError)]` gains one variant attribute, `#[error(rejection)]`
  (alias: `#[error(transparent)]` over a single field whose type is
  `Rejection`), on a variant holding exactly one `Rejection`. It takes no
  `status`/`message` (both come from the carried `Rejection`; remap through
  `ErrorSchema::status_of`), and only one variant may carry it (`From` would be
  ambiguous) — both are compile errors. The derive then emits `From<Rejection>`
  and `ErrorSchema`: `status_of` = the default table, `body_schema` /
  `body_schema_for` delegate to `HttpError` (the rejection variant renders the
  `HttpError` bodies; the `JsonSchema` probe for a custom body is P2),
  `extra_statuses` = the sorted, de-duplicated fixed statuses of the other
  variants with humanized names, `opaque_passthrough() = true`. An enum with
  exactly one `#[error(transparent)]` variant over `HttpError` and no rejection
  variant inherits the same impls (`E::Variant(HttpError::from(r))`); two such
  variants emit nothing. An enum without any of this does not implement
  `From<Rejection>`, and using it as `error = E` is a compile error (P1).
- A hand-written envelope implements the three traits directly; the schema side
  uses `schemars` through the existing optional path (`r2e_schemars_path()`), so
  an app without the `openapi` feature pays nothing.

### 3.4 Projection levels

| Level | Declared by | Applies to |
|---|---|---|
| route | the handler's return type `Result<T, E>`, `E: From<Rejection> + IntoHttpResponse + ErrorSchema` (autoref probe at codegen, no attribute) | every failure of that route: request data, guards, parameters, body, validation, managed, handler `Err` |
| app | `AppBuilder::error_projection::<E>()` → `ErrorProjector` bean (P1); 404/405/413/panic wiring in P4 | routes whose return type declares no envelope (`Json<T>`, `Result<T, HttpError>`, `Result<T, E>` with a plain `E`), SSE/WS routes, and the non-route failures |
| framework | — | `HttpError` when no `ErrorProjector` bean is provided |

The app level is an `ErrorProjector(Arc<dyn Fn(Rejection) -> Response>)` bean
built from `E` (`status_of` + `From` + `into_http_response`) so the generated
entry fns and the router-wide layers (catch-panic, fallback, body-limit) can
call it without a type parameter; `project_default(rejection, &state)` picks
the bean when present, else `HttpError`. A handler `Result<T, E>` whose `E`
only implements `IntoHttpResponse` keeps rendering its own `Err` as today and
uses the app level for framework failures.

## 4. Codegen: the single projection point

### 4.1 Take over extraction (option B, locked)

Today axum extracts the handler's argument tuple itself, which is exactly why
exit door 1 is invisible. Option A (wrap each param as `Result<T, T::Rejection>`
and `match` in the invoke fn) was rejected: axum extracts every tuple element
before calling the handler, so the body is read and parsed before the identity
is checked, and it keeps two codegen shapes to maintain.

Option B: the generated closure takes `(State<S>, Request)` only, and the
generated body performs extraction itself, in a deliberate order, each step
short-circuiting through the projector.

```text
split Request into (Parts, Body)
pre-auth guards                 (PreAuthGuard, head only)
request data                    identity + #[inject(request)] via FromRequestPartsVia
guards                          controller-level then method-level
non-body params                 FromRequestParts<S>, in signature order
body param                      FromRequest<S>, last param only (mirrors axum's rule)
garde validation
managed acquire
handler (through the interceptor chain, unchanged)
into_response
managed finalize                errors → Rejection → projector
```

Every `Err(x)` on the way becomes:

```rust
return Rejection::from(x).project::<E>();   // or the autoref probe → Opaque
// = status_of remap (only when it differs from the default), opaque passthrough,
//   E::from(r).into_http_response(), hub headers added with or_insert
```

The handler's own `Err(E)` reaches the same `into_http_response`, so there is
exactly one rendering site.

What this removes:

- `__R2eRequestData<M>` no longer needs an axum `FromRequestParts` impl: it is
  built by generated code from `FromRequestPartsVia` calls. One named bridge
  row gone (`r2e-core/src/web/extract.rs`, table in the module docs).
- `Via<T, M>` is no longer a closure parameter; identity params are extracted
  by the same generated call. `Via` and `BeanExtract` stay for hand-written
  handlers merged through `merge_router`.
- `ParamsRejectionFormat` (global JSON/plain-text switch for `#[derive(Params)]`)
  and the `params.rejection-format` key: the projector decides. Removed.

What stays: the marker generics on the `Controller` impl, the decorator sets,
the interceptor chain and its `into_response`-after-chain rule, the managed
RAII guard. SSE and WS handlers use the same extraction routine (the WS
preflight already runs guards before the upgrade; it switches to `Rejection`).

### 4.2 Typed exits for guards, validation, managed

| Surface | Today | New |
|---|---|---|
| `Guard::check` | `Result<(), Response>` | `Result<(), Rejection>` — no associated type; typed errors (`RolesDenied`, `RateLimited`, `GuardError`) flow through `?` via `From`; `Response: Into<Rejection>` (Opaque) keeps custom-body guards working with `.into()` |
| `PreAuthGuard::check` | same | same |
| `__maybe_validate` | `Result<(), Box<Response>>` | `Result<(), Box<Rejection>>` |
| `ManagedResource::Error` | `Into<Response>` | `Into<Rejection>`; `ManagedErr<E: Into<Rejection>>` |
| `RolesGuard` / `AllRolesGuard` / `RateLimitGuard` / `FgaGuard` | `GuardError` → `Response` | `RolesDenied` / `RateLimited { retry_after }` / `FgaDenied`, each `From<_> for Rejection` |
| `GrpcGuard<I>` | separate trait returning `tonic::Status` | **removed**; gRPC runs `Guard<I>` and maps with `r2e_grpc::rejection_to_status(rejection)` (free fn — `From<Rejection> for tonic::Status` is an orphan) |

### 4.3 No attributes: the return type is the declaration

- No `#[error(E)]`, no `#[routes(error = E)]`. The generated entry fn probes
  the route's return type with an autoref specialization
  (`r2e_core::error::projection::{ProjectionProbe, ProjectEnvelope,
  ProjectFallback}`): `Result<T, E>` with `E: From<Rejection> +
  IntoHttpResponse + ErrorSchema` projects through `E`; anything else calls
  `project_default(rejection, &state)` (the `ErrorProjector` bean, else
  `HttpError`).
- Rationale (user decision 2026-10-09): a route that returns `Json<T>` is
  declared infallible by its author; its framework failures still render, with
  the app-level envelope. If the route must speak `E`, declare `Result<T, E>`.
  `error_projection::<E>()` is the JAX-RS/Quarkus `ExceptionMapper`.
- `#[anonymous]` routes project the same way (they can fail on body, path,
  guards). SSE and WS routes always use the app level.
- Order inside the entry fn: pre-auth guards (controller then route) → request
  data → identity param → guards (controller then route; skipped on
  `#[anonymous]`) → head parameters → body parameter (last, `FromRequest`) →
  garde validation → managed acquire → interceptors → handler → managed
  finalize. Guards therefore run **before** the route's own parameters, and the
  body is never read before identity and guards have passed.

## 5. Transports

- **MCP**: `impl From<Rejection> for McpError` by kind (Unauthenticated →
  `Unauthorized`, Forbidden → `Forbidden`, NotFound → `NotFound`, Invalid* /
  Validation → `InvalidParams`, Internal/Unavailable/Timeout → `Internal`,
  Conflict/RateLimited → `Tool { data: details }`, else by status).
  `guard_rejection_to_error` (the body-read fold) is deleted; the tool codegen
  calls `McpError::from(rejection)`. MCP tools that reuse HTTP guards get the
  mapping for free.
- **gRPC**: `r2e_grpc::rejection_to_status(Rejection) -> tonic::Status` by
  kind (Unauthenticated → `UNAUTHENTICATED`, Forbidden → `PERMISSION_DENIED`,
  Invalid*/Validation → `INVALID_ARGUMENT`, NotFound → `NOT_FOUND`, Conflict →
  `ABORTED`, RateLimited/PayloadTooLarge → `RESOURCE_EXHAUSTED`, Unavailable →
  `UNAVAILABLE`, Timeout → `DEADLINE_EXCEEDED`, Internal → `INTERNAL`, other
  kinds by status via `code_from_status`); message → status message, headers
  → response metadata. A free function, not `From<Rejection> for Status`: both
  types are foreign to `r2e-grpc` (orphan rule). `GrpcGuard`, `GrpcGuardContext`,
  `GrpcRolesGuard`, `GrpcRoleBasedIdentity` are removed; `r2e_grpc::guard_context`
  builds the shared `GuardContext` from the `tonic::Request` (metadata as
  `HeaderMap`, `POST`, uri `/`, empty path params) and the gRPC codegen runs
  `Guard<I>::check` (controller sites, then method sites) and maps. Guards are
  transport-agnostic.
- **gRPC identity**: `#[inject(identity)]` method parameters (required or
  `Option<T>`) are wired through a new `r2e_grpc::GrpcIdentity: Identity`
  trait — `type Spec: DecoratorSpec` + `extract(&Spec::Product, &MetadataMap)
  -> Result<Self, Rejection>` + `extract_optional` (absent `authorization` ⇒
  `None`). The spec joins the service's `EndpointDeps` and config validation
  like a decorator spec, so the validator bean is checked at
  `register_grpc_service`. `r2e-security/grpc` implements it for
  `AuthenticatedUser` with `JwtIdentitySpec` (product `Arc<JwtClaimsValidator>`).
  Struct-level identity on a gRPC service is a compile error, as is a
  `REQUIRES_IDENTITY` guard on a method with no identity parameter.

## 6. OpenAPI

`RouteInfo` gains (shipped shape, P2):

```rust
pub struct RouteInfo {
    // …
    /// Failure kinds the macro inferred from the signature and decorators,
    /// completed at metadata time by what only the compiled program knows.
    pub rejection_kinds: Vec<RejectionKind>,
    /// The envelope of a `Result<T, E>` return type (`E: ErrorSchema`),
    /// captured through the P1 `ProjectionProbe`; `None` = application
    /// projection. Not serialized.
    pub error_schema: Option<ErrorSchemaInfo>,
}
```

`ErrorSchemaInfo` (`r2e_core::error`) is the `Copy` capture of an
`ErrorSchema` impl (`of::<E>()`: type name + fn pointers for `status_of`,
`body_schema`, `body_schema_for`, `extra_statuses`, `opaque_passthrough`);
`ErrorProjector` carries one too (`projector.schema()`), which is how the
OpenAPI plugin learns the application envelope from the bean graph.

Inference table (`controller_impl.rs`, `static_rejection_kinds` +
`rejection_kinds_expr`):

| Signature / decorator | Kinds | When |
|---|---|---|
| `Json<T>` body | MissingContentType, PayloadTooLarge, BodyRead, MalformedBody, InvalidBody | macro |
| `TypedMultipart<T>` / `Multipart` | UnsupportedMediaType, PayloadTooLarge, MalformedBody | macro |
| `Form<T>` (last param) | UnsupportedMediaType, PayloadTooLarge, InvalidForm | macro |
| `Bytes` / `String` (last param) | PayloadTooLarge, BodyRead (+ MalformedBody for `String`) | macro |
| custom last param with `RequestBodySchema` | its `rejection_kinds()` | runtime probe |
| `Path<_>` | InvalidPath | macro |
| `Query<_>` | InvalidQuery | macro |
| `#[derive(Params)]` fields | InvalidPath / InvalidQuery / InvalidHeader by location | runtime (`ParamInfo`) |
| required identity param | Unauthenticated | macro |
| required struct identity, route not `#[anonymous]` | Unauthenticated | runtime (`HAS_STRUCT_IDENTITY`) |
| `#[roles]`, `#[all_roles]`, any guard | Forbidden | macro |
| guard whose spec type name contains `RateLimit` (`RateLimit`, `PreRateLimit`, `Configured*`, `RateLimitGuard`, …) | RateLimited | macro (`spec_type_of`) |
| param type implementing `garde::Validate` (inner of `Json`/`Query`/`Path`/`Form`) | Validation | runtime autoref probe |
| always | Internal | macro |

Controller-level post-auth guards fold into non-`#[anonymous]` routes,
pre-auth guards into every route (the same rule as their execution). SSE/WS
routes get the identity/guard kinds and `Internal`, `error_schema: None`.

Builder (`r2e-openapi/src/builder.rs`): per route, envelope =
`route.error_schema` → `config.error_schema` → `HttpError`; one response per
distinct `status_of(kind)` with `body_schema_for(kind)` (else
`body_schema()`), several bodies on one status as `oneOf`, then
`extra_statuses`; the success status wins a collision. Components the
envelopes declare are inserted after `schema_overrides` with `or_insert`, so
overrides still win. The hardcoded 401/403/500/400 blocks and the
`ErrorResponse` / `ValidationErrorResponse` / `FieldError` inserts are gone:
`HttpError`'s `ErrorSchema` impl provides the first two (`Validation` →
`ValidationErrorResponse` with inline items). Consequences for the default
spec: JSON-body routes now list 413/415/422 (and 400 only when a kind maps
there), `Path`-param routes list 400, and a route with no failure kind but
`Internal` lists 500 only. The plugin (`ext.rs`) reads the `ErrorProjector`
bean in `after_routes` to fill `OpenApiConfig::error_schema` unless
`with_error_schema::<E>()` was called.

`RequestBodySchema` (`r2e_core::di::meta`): `content_type()`, `body_schema()`,
`rejection_kinds()`; probed by autoref on the handler's last extracted
parameter when it is none of the name-detected extractors. **Deviation from
the first draft:** it is *not* implemented for `Json<T>` / `TypedMultipart<T>`
— r2e-core has no `schemars`, so the built-ins stay name-based in the macro
(`extract_body_type_info`) and only app extractors use the trait.

## 7. What it looks like for llm-engine-rust

```rust
#[derive(Debug)]
pub struct OpenAiError(AppError);

impl From<Rejection> for OpenAiError {
    fn from(r: Rejection) -> Self {
        let (ty, code) = match r.kind {
            RejectionKind::Unauthenticated => ("authentication_error", "invalid_api_key"),
            RejectionKind::Forbidden       => ("permission_error", "insufficient_permissions"),
            RejectionKind::RateLimited     => ("rate_limit_error", "rate_limit_exceeded"),
            RejectionKind::PayloadTooLarge => ("invalid_request_error", "payload_too_large"),
            k if k.is_client_error()        => ("invalid_request_error", "invalid_body"),
            _                              => ("server_error", "internal"),
        };
        OpenAiError(AppError::wire(r.status, ty, code, r.message, r.headers))
    }
}

impl ErrorSchema for OpenAiError {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind { RejectionKind::InvalidBody => StatusCode::BAD_REQUEST, k => k.default_status() }
    }
    fn body_schema() -> Option<(String, Value)> { Some(schema_of::<OpenAiErrorBody>("OpenAiError")) }
    fn extra_statuses() -> Vec<(StatusCode, &'static str)> { vec![(StatusCode::BAD_GATEWAY, "Upstream failed")] }
}

#[routes]
impl ChatCompletionsController {
    #[post("/v1/chat/completions")]            // envelope = the return type's `E`
    async fn chat(&self, user: UserContext, Json(req): Json<ChatRequest>)
        -> Result<ChatReply, OpenAiError> { … }
}
```

`ProxyJson`, `BodyError`, `MaybeAuthenticated`, the four `match` blocks,
`PROXY_ERROR_ENVELOPES` and the `docs.rs` patch all go away; the 413 and
`Retry-After` behaviour is kept because `Rejection` carries them.

## 8. Breaking changes

- `Guard::check`, `PreAuthGuard::check` return `Result<(), Rejection>`.
- `ManagedResource::Error: Into<Rejection>` (was `Into<Response>`).
- `ParamsRejectionFormat` and `params.rejection-format` config removed.
- `r2e_mcp::guard::guard_rejection_to_error` removed (`McpError: From<Rejection>`).
- `r2e_grpc::GrpcGuard` / `GrpcGuardContext` / `GrpcRolesGuard` /
  `GrpcRoleBasedIdentity` removed in favour of `Guard<I>` + `guard_context` +
  `rejection_to_status` (`extract_bearer_token` keeps its statuses and
  messages: it is `bearer_token` projected).
- `__R2eRequestData<M>` no longer implements `FromRequestParts` (internal, but
  the bridge table in `web/extract.rs` is public doc).
- The default spec lists 413/415/422 for body routes.
- `ParamError` gains `location: ParamLocation` (struct-literal construction
  outside the derive breaks).
- `Rejection` is new; its `Display` is the bare message.
- Version: **0.5.0**, not the 0.4.1 the ticket assumed.

Not breaking: handler signatures, `HttpError` bodies and statuses,
`#[derive(ApiError)]` without the new attribute, hand-written
`IntoHttpResponse` error types, interceptors.

## 9. Phases

One PR per phase, sequential, same branch. Each phase ships green on
`cargo test --workspace`, `cargo check -p r2e-core --features dev-reload`,
`scripts/check-dep-boundary.sh`, `scripts/check-source-boundary.sh`.

| Phase | Scope | Tests |
|---|---|---|
| **P0** core types | `Rejection`, `RejectionKind`, `ErrorSchema`, every `From<X> for Rejection`, `From<Rejection> for HttpError` + `ErrorSchema for HttpError`, `From<Rejection> for Response`, typed guard errors (`RolesDenied`, `RateLimited`, `FgaDenied`), `TenantError::into_rejection`, `ParamError::location`, derive `#[error(rejection)]`. Guards still return `Response` (they render through the typed errors); no codegen change. **Shipped** (PR for P0). | `r2e-core/tests/http/rejection.rs` (new `mod`): kind → status table, every `From` impl, `HttpError` projection byte-equal to today's `into_response`, coherence `E::from(r).status() == E::status_of(r.kind)`; derive cases in `tests/http/api_error.rs`; `r2e-compile-tests` for derive misuse; owning-crate tests (`r2e-security/tests/{error,guards}.rs`, `r2e-rate-limit/tests/guard.rs`, `r2e-openfga/tests/guard.rs`, `r2e-tenant/tests/tenant/error.rs`) |
| **P1** single projection point | Option B extraction in `handlers.rs` (route, SSE, WS): one `(State, Request)` entry fn per endpoint, pre-auth guards as its first step (middleware layer removed), `RequestData<S>` replacing the `FromRequestParts` bridge, guards/pre-guards returning `Result<(), Rejection>`, `ManagedResource::Error: Into<Rejection>`, envelope inferred from the return type (autoref probe, no attributes), `AppBuilder::error_projection::<E>()` + `ErrorProjector` bean, `ParamsRejectionFormat` removal. **Shipped** (PR for P1). | `tests/http/projection.rs`: malformed body, missing content-type, bad path, failed identity, guard, garde, managed acquire and finalize all answer in `E`'s envelope with the right status and headers; identity failure never reads the body (counting body reader); every existing `r2e-core/tests/http` + `tests/decorators` test unchanged with the default projector |
| **P2** OpenAPI | `rejection_kinds` + `error_schema` on `RouteInfo` (`has_auth` removed), `ErrorSchemaInfo`, `ErrorProjector::schema()`, builder rewrite, plugin reads the projector bean, `OpenApiConfig::with_error_schema`, `RequestBodySchema` (app extractors only). **Shipped** (PR for P2). | `r2e-openapi/tests/errors.rs`: per-kind statuses/bodies, `oneOf` on 400, remap 422→400 shows 400 only, extra 502, route envelope over config envelope, override precedence, plugin with/without projector; `r2e-core/tests/controller/error_meta.rs`: the inference table (body, Path/Query/Params, identity param vs struct vs `#[anonymous]`, guards vs rate-limit guards, garde, SSE), envelope capture, custom body extractor |
| **P3** transports | `From<Rejection> for McpError`, `rejection_to_status` (free fn, orphan rule), MCP body-read fold deleted, `GrpcGuard` family deleted, gRPC codegen runs `Guard<I>` through `guard_context`, `GrpcIdentity` + `JwtIdentitySpec` for `#[inject(identity)]` parameters (deps + config checked at registration), `#[guard]`/`#[roles]`/`#[all_roles]` allowed on gRPC methods and impl blocks. **Shipped** (PR for P3). | `r2e-mcp/tests/server/rejection.rs` (kind table), `r2e-grpc/tests/guard.rs` (kind → code table, metadata, `guard_context`, end-to-end `Guard<I>`), `r2e-security/tests/grpc.rs` (`GrpcIdentity for AuthenticatedUser`), `examples/example-grpc/tests/grpc_guards.rs` (real tonic round-trips), compile tests `grpc/pass/grpc_guards.rs`, `grpc/fail/grpc_roles_without_identity.rs`, `grpc/fail/grpc_identity_missing_validator.rs` |
| **P4** app level | `AppBuilder::error_projection::<E>()`, routed into catch-panic, fallback 404/405, body-limit 413. | `tests/http/panic.rs` + new `tests/runtime/fallback.rs`: panic, unknown route, wrong method, oversized body answer in `E`'s envelope |
| **P5** docs + release | `llm/error-handling.md`, `openapi.md`, `guards.md`, `managed-resources.md`, `validation.md`, `grpc.md`, `mcp-server.md`, `coming-from-axum.md`; `docs/claude/error-handling.md`, `guards-interceptors.md`, `architecture.md` bridge table, `configuration.md`, `prelude-features.md`, `docs/features/02-validation.md`; `r2e-grpc/README.md`; CHANGELOG "Breaking"; `check-llm-docs.sh --update`; bump 0.5.0 (publish via CI only). | `cargo test -p llm-doctests` |
| **P6** | llm-engine-rust migration (its own repo). | — |

P2's `error_schema` capture reuses P1's `ProjectionProbe` (`error_schema()` on `ProjectEnvelope`/`ProjectFallback`).

## 10. Decisions (locked 2026-10-09)

1. **Option B**: generated code owns extraction; identity and guards run before
   the body is read; one codegen path.
2. **From/Into model**: `Rejection` is the hub; faults convert with
   `From<X> for Rejection`; the envelope is `E: From<Rejection> +
   IntoHttpResponse + ErrorSchema`; wires are `From<Rejection>`. No
   `IntoRejection`, no `ApiError::from_rejection`.
3. **Guards return `Result<(), Rejection>`**, no associated error type; typed
   guard errors flow through `?`. Same for `PreAuthGuard`.
   `ManagedResource::Error: Into<Rejection>`.
4. **`ErrorSchema` is mandatory** for every projector type.
5. **Strict statuses**: `E::status_of(kind)` is written into
   `Rejection::status` before `E::from`, and the OpenAPI builder reads the same
   function; `From<Rejection>` impls use `rejection.status`.
6. **`ParamsRejectionFormat` removed.**
7. **0.5.0**, phases P0 → P5 in this repo.

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
3. Projection has levels: per route `#[error(E)]` > per controller
   `#[routes(error = E)]` > app `error_projection::<E>()` for non-route failures
   > the framework default `HttpError`. The default reproduces 0.4.0 byte for
   byte, so the attribute is purely additive for existing apps.
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
| route | `#[error(E)]` on a route method | that route |
| controller | `#[routes(error = E)]` | every route, SSE and WS of the impl |
| app | `AppBuilder::error_projection::<E>()` (phase 4) | the non-route failures: 404, 405, 413 body limit, panic 500 |
| framework | — | `HttpError` |

The app level is stored as an `Arc<dyn Fn(Rejection) -> Response>` bean built
from `E` (`status_of` + `From` + `into_http_response`) so the router-wide
layers (catch-panic, fallback, body-limit) can call it without a type
parameter. Controllers that declare nothing resolve to `HttpError` statically
(the macro cannot see the app-level type); the two defaults stay explicit.

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
| `GrpcGuard<I>` | separate trait returning `tonic::Status` | **removed**; gRPC runs `Guard<I>` and maps with `Status::from(rejection)` |

### 4.3 Attributes

- `#[routes(error = E)]` on the impl; `#[error(E)]` on a route, SSE or WS
  method. Both are a path to a type implementing
  `From<Rejection> + IntoHttpResponse + ErrorSchema`.
- Compile errors, all pointing at the attribute: `E` misses one of the three
  traits (each with its own `on_unimplemented` note); `#[error]` on a
  non-route method; `error =` given twice.
- `#[anonymous]` routes still project through `E` (they can fail on body,
  path, guards).

## 5. Transports

- **MCP**: `impl From<Rejection> for McpError` by kind (Unauthenticated →
  `Unauthorized`, Forbidden → `Forbidden`, NotFound → `NotFound`, Invalid* /
  Validation → `InvalidParams`, Internal/Unavailable/Timeout → `Internal`, else
  `Tool`). `guard_response_to_error` and its body read are deleted. MCP tools
  that reuse HTTP guards get the mapping for free.
- **gRPC**: `impl From<Rejection> for tonic::Status` by kind (Unauthenticated →
  `UNAUTHENTICATED`, Forbidden → `PERMISSION_DENIED`, Invalid*/Validation →
  `INVALID_ARGUMENT`, NotFound → `NOT_FOUND`, RateLimited →
  `RESOURCE_EXHAUSTED`, Unavailable → `UNAVAILABLE`, Timeout →
  `DEADLINE_EXCEEDED`, Internal → `INTERNAL`). `GrpcGuard` is removed; the
  gRPC codegen calls `Guard<I>::check` and maps. Guards become
  transport-agnostic.

## 6. OpenAPI

`RouteInfo` gains:

```rust
pub struct RouteInfo {
    // …
    /// Failure kinds the macro inferred from the signature and decorators.
    pub rejection_kinds: &'static [RejectionKind],
    /// The route's envelope documentation, captured from `E: ErrorSchema`.
    pub error_schema: fn() -> ErrorSchemaInfo, // { status_of: fn(RejectionKind) -> StatusCode, body, body_for, extra }
}
```

Inference table (macro side, `controller_impl.rs`):

| Signature / decorator | Kinds |
|---|---|
| body param (`Json`, `TypedMultipart`, custom `RequestBodySchema`) | MissingContentType, MalformedBody, InvalidBody, PayloadTooLarge |
| `Path<_>` or path symbols | InvalidPath |
| `Query<_>` / `Params` with query fields | InvalidQuery |
| `Form<_>` | InvalidForm |
| required identity (struct or param) | Unauthenticated |
| `#[roles]`, `#[all_roles]`, any guard | Forbidden |
| `RateLimitGuard` / `PreRateLimit` | RateLimited |
| garde `Validate` on a param | Validation |
| always | Internal |

Builder (`r2e-openapi/src/builder.rs`): for each inferred kind, emit
`status_of(kind)` with `body_schema_for(kind)` or `body_schema()`, merge kinds
that land on the same status, then add `extra_statuses`, and register the
components. The hardcoded `ErrorResponse` / `ValidationErrorResponse` blocks
become `HttpError`'s `ErrorSchema` impl, so the default spec is unchanged except
that it now also lists 415 and 422 for JSON bodies and 413 for bodies, which
the runtime already emits. A projector that remaps 422 → 400 documents 400
only, because both sides call the same `status_of`.

`RequestBodySchema` (from the ticket): a trait probed like `MultipartSchema`,
implemented for `Json<T>` and `TypedMultipart<T>`, implementable by an app
extractor; the name-based `extract_body_type_info` stays as the fallback.

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

#[routes(error = OpenAiError)]
impl ChatCompletionsController {
    #[post("/v1/chat/completions")]
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
- `r2e_mcp::guard::guard_response_to_error` removed.
- `r2e_grpc::GrpcGuard` / `GrpcRolesGuard` removed in favour of `Guard<I>`.
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
| **P1** single projection point | Option B extraction in `handlers.rs` (route, SSE, WS), typed guards/validation/managed, `#[routes(error)]` + `#[error]`, `ParamsRejectionFormat` removal. | compile tests (missing trait, misplaced `#[error]`); `tests/http/projection.rs`: malformed body, missing content-type, bad path, failed identity, guard, garde, managed acquire and finalize all answer in `E`'s envelope with the right status and headers; identity failure never reads the body (counting body reader); every existing `r2e-core/tests/http` + `tests/decorators` test unchanged with the default projector |
| **P2** OpenAPI | `rejection_kinds` + `error_schema` on `RouteInfo`, builder rewrite, `RequestBodySchema`. | spec snapshot: default projector (unchanged modulo 413/415/422), custom projector remapping 422→400 shows 400 only, extra 502 listed; custom body extractor documented |
| **P3** transports | `From<Rejection>` for `McpError` and `tonic::Status`; delete the MCP body-read fold; delete `GrpcGuard`, gRPC codegen runs `Guard<I>`. | existing MCP/gRPC guard tests pass by kind; mapping table tests |
| **P4** app level | `AppBuilder::error_projection::<E>()`, routed into catch-panic, fallback 404/405, body-limit 413. | `tests/http/panic.rs` + new `tests/runtime/fallback.rs`: panic, unknown route, wrong method, oversized body answer in `E`'s envelope |
| **P5** docs + release | `llm/error-handling.md`, `openapi.md`, `guards.md`, `managed-resources.md`, `validation.md`, `grpc.md`, `mcp-server.md`, `coming-from-axum.md`; `docs/claude/error-handling.md`, `guards-interceptors.md`, `architecture.md` bridge table, `configuration.md`, `prelude-features.md`, `docs/features/02-validation.md`; `r2e-grpc/README.md`; CHANGELOG "Breaking"; `check-llm-docs.sh --update`; bump 0.5.0 (publish via CI only). | `cargo test -p llm-doctests` |
| **P6** | llm-engine-rust migration (its own repo). | — |

P0 and P2's `RequestBodySchema` are independent of P1 and can land first.

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

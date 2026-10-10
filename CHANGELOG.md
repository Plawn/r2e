# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

> **Tags vs versions.** A tag `vX.Y.Z` means "`X.Y.Z` is published on
> crates.io from this commit" — nothing else creates one. A release is a
> `release: X.Y.Z` PR made by `scripts/bump-version.sh` (workspace version, pins,
> lockfile, this file's `[Unreleased]` → `[X.Y.Z]`); once it is merged and Tests
> pass, `.github/workflows/release.yml` publishes, tags, and uses the `[X.Y.Z]`
> section below as the GitHub release notes.
>
> Before that, tags were a release counter pushed on every merge, detached from
> what crates.io serves: `v0.2.132`–`v0.2.163` contain workspace version
> `0.3.0` (the 0.3 plugin-API rework ships from **`v0.2.140`** onward, see
> [`docs/migration/plugin-api.md`](docs/migration/plugin-api.md)), and
> `v0.3.3`–`v0.3.15` are snapshots of master that were never published —
> crates.io went from `0.3.2` straight to the next release.

## [Unreleased]

## [0.6.0] - 2026-10-10

### Breaking

OpenAPI bodies are now documented **only** through `RequestBodySchema` /
`ResponseBodySchema`: the `#[routes]` macro probes the body parameter's type
and the return type for the traits instead of matching `Json<T>` /
`JsonResult<T>` / `Bytes` / … by name, and R2E implements the traits for its
own types. Handlers using framework types need no change; the developer
migration is [`docs/migration/response-schema.md`](docs/migration/response-schema.md).

- **Breaking: `RouteInfo` reshaped.** `request_body_type`,
  `request_body_schema`, `request_body_content_type` and
  `request_body_required` are replaced by
  `request_body: Option<RequestBody { content_type, schema, required }>` plus
  `request_body_unmapped: Option<String>`; `response_type` and
  `response_schema` are replaced by `response_contents: Vec<ResponseContent>`
  (`response_unmapped` is kept). Struct literals add
  `request_body: None, request_body_unmapped: None, response_contents: Vec::new()`.
- **Breaking: `SchemaGap` variants.** `SchemalessRequestBody` and
  `SchemalessResponseBody` are gone — a schemaless body renders from its media
  type (`text/*` → string, `application/octet-stream` → binary string, form
  media types → object) and is not a gap. `MissingRequestBody { type_name }`
  is new; `MissingResponseBody` and `ErrorBodyInlined` are unchanged.
- **Breaking: `Json<T>` with `T: !JsonSchema` is undocumented.** It was
  rendered as a generic `object`; it now has no body in the spec and is
  warned about once at boot (`MissingRequestBody` / `MissingResponseBody`).
  Derive `schemars::JsonSchema` on the DTO.
- **Breaking: component names are schemars' `schema_name()`** — `Vec<User>`
  is `Array_of_User` (was `Vec_User`), `Option<User>` is `Nullable_User`.
  Generated clients and tests pinning `components/schemas` keys must follow.
- **Breaking: `String` returns document a body.** `String` / `&'static str` /
  `Cow<'static, str>` return types (and `Html<T>`, `Bytes`, `Vec<u8>`) carry
  their media type through their `ResponseBodySchema` impl instead of "no
  schema".
- **Breaking: `MultipartSchema::schema_name()`** is required (the
  `#[derive(FromMultipart)]` emits it; hand-written impls add it).
- **Breaking: `schema_of` moved** to `r2e_core::di::meta::schema_of` behind
  the new `r2e-core` feature `openapi` (`r2e-openapi` enables it and keeps
  the `r2e_openapi::schema_of` re-export).
- **Breaking: `#[returns(T)]` widened.** `T` is probed for
  `ResponseBodySchema` first, then as `Json<T>`; existing `JsonSchema` uses
  are unchanged, a custom multi-media-type response type now works too.

### Added

- **`RequestBodySchema` / `ResponseBodySchema` for every framework type**
  (`r2e_core::web::body_schema`): `Json<T>`, `Form<T>`, `Bytes`, `String`,
  `Multipart`, `TypedMultipart<T>` on the request side; `Json<T>`, `String`,
  `&'static str`, `Cow<'static, str>`, `Html<T>`, `Bytes`, `Vec<u8>`, `()`,
  `StatusCode`, `Redirect`, `Sse<S>` on the response side, with blanket
  `Result<T, E>` → `T` and `(StatusCode, T)` / `(HeaderMap, T)` /
  `(StatusCode, HeaderMap, T)` → `T`. Any alias (`ApiResult<T>`,
  `anyhow::Result<T>`) is therefore documented. `Option<..>` around a body
  extractor → `required: false`.
- **`ResponseBodySchema`** (`r2e_core::di::meta`): a custom response type
  lists the media types it can be served as (`ResponseContent::json` /
  `event_stream` / `text` / `html` / `binary` / `new`); the OpenAPI builder
  documents one `content` entry per media type under the success status. A
  handler that answers JSON or SSE depending on the request can return a typed
  enum instead of `Response` and keep a documented body.
- **`RequestBodySchema::body_schema()` defaults to `None`**, so a schemaless
  extractor only declares `content_type()` and `rejection_kinds()`.
- **`RequestBody`**, **`ResponseContent::html()` / `::binary()`** and
  **`OCTET_STREAM`** in `r2e_core::di::meta`; **`r2e-core` feature `openapi`**
  (schemars) gating `schema_of` and the `Json<T>` schemas — without it the
  `Json<T>` impls still document media type and rejection kinds, schema-less.
- **Boot warning `SchemaGap::MissingRequestBody`** for a body-position
  parameter without `RequestBodySchema`, the request-side twin of
  `MissingResponseBody`.

## [0.5.0] - 2026-10-10

### Breaking

Task #1072 (error projection layers) is the 0.5.0 break: one typed `Rejection`
per failure, projected once per route through an error envelope. Each item is
detailed under *Added*; the step-by-step developer migration is
[`docs/migration/error-projection.md`](docs/migration/error-projection.md).

- **Breaking: extractor rejections render as the envelope's JSON** (task
  #1072). Every extractor rejection — `Json`/`Path`/`Query`/`Form`/header/
  multipart failures, a missing or unsupported `content-type`, a 413 body
  limit — is now projected through the route's envelope, so with the default
  `HttpError` it answers `{"error": "..."}` (`application/json`) where 0.4
  returned axum's `text/plain` body. Status codes are unchanged; `HttpError`'s
  own bodies stay byte-equal to 0.4. Clients parsing those plain-text bodies
  must be updated.
- **Breaking: `HttpError::Validation` is a struct variant**
  `Validation { status: StatusCode, response: ValidationErrorResponse }` (was
  the tuple `Validation(ValidationErrorResponse)`). Build it with
  `HttpError::validation(response)` (status 400); match with
  `HttpError::Validation { response, .. }`. The carried status keeps an
  envelope's `status_of(Validation)` remap (e.g. 422) through
  `From<Rejection> for HttpError` and back. The default 400 body is unchanged.
- **Breaking: guards return `Result<(), Rejection>`** (task #1072, phase P1).
  `Guard<I>::check` and `PreAuthGuard::check` no longer build a `Response`;
  they return a `Rejection` (`Rejection::forbidden(..)`,
  `Rejection::unauthenticated()`, or a typed error through `?`/`From`) that the
  route projects through its envelope. `GuardContext::parse_path_param` returns
  `Result<T, GuardError>`. Pre-auth guards are the first step of the route's
  entry fn, not a middleware layer.
- **Breaking: `ManagedResource::Error: Into<Rejection>`** (was
  `Into<Response>`). Acquire/finalize failures are projected like any other
  rejection; `HttpError` still qualifies. `ManagedErr<E>` still requires
  `E: Into<Rejection>`; an error that was only `Into<Response>` must be mapped
  (e.g. to `HttpError`) first.
- **Breaking: `ParamsRejectionFormat` and `server.params-rejection-format`
  removed.** `#[derive(Params)]` failures always render through the route's
  envelope (default `HttpError`, same body as the previous default).
- **Breaking: `Via<T, M>` removed.** The generated handlers resolve bean-backed
  extractors inline; hand-written handlers keep `BeanExtract<T, I>`. The
  `ViaAxum` bridge now requires the axum rejection to convert `Into<Rejection>`
  (every axum built-in does).
- **Breaking: the gRPC guard family is gone** (task #1072, phase P3).
  `GrpcGuard`, `GrpcGuardContext`, `GrpcRolesGuard` and `GrpcRoleBasedIdentity`
  are removed from `r2e-grpc` (and its prelude); write `Guard<I>` impls and put
  them on the method or impl block with `#[guard]` — they now work on HTTP,
  gRPC and MCP alike. Manual `GrpcIdentityExtractor::extract_claims` wiring
  still compiles but is no longer needed: use an `#[inject(identity)]`
  parameter.
- **Breaking: `r2e_mcp::guard::guard_response_to_error` removed**; the
  generated tool code uses `McpError::from(rejection)`.
- **Breaking: `RouteInfo.has_auth` removed** (task #1072, phase P2) in favour
  of `rejection_kinds`; `RouteInfo` literals need the two new fields. The
  OpenAPI spec no longer hardcodes 401/403/500/400: default-envelope routes
  now list 415/413/422 for JSON bodies and 400 for path/query parameters
  (what the runtime already answered), and the `FieldError` component is gone
  (`ValidationErrorResponse.details` items are inline). `RejectionKind` is
  `Serialize`.
- **Breaking: `ParamError` gains `location: ParamLocation`** (`Path` / `Query` /
  `Header`), so a `#[derive(Params)]` failure converts into the right
  `RejectionKind`. Code that builds a `ParamError` by struct literal or
  destructures it exhaustively (`let ParamError { message } = e`) must name
  the new field; hand-written `PrefixedExtract` impls (below) now return it.
- **Breaking: `PrefixedExtract::extract_prefixed` returns
  `Result<Self, ParamError>`** (was `Result<Self, Response>`). A hand-written
  impl (nested `#[derive(Params)]` support) returns a `ParamError` with its
  `location` instead of a rendered response.
- **Breaking: `#[derive(Params)]` rejects with `ParamError`** —
  `<T as FromRequestParts<S>>::Rejection` is `ParamError` (was `Response`).
  Code naming the rejection type or calling `.into_response()` on it still
  works (`ParamError: IntoHttpResponse`); code matching it as a `Response`
  must convert first.
- **Breaking: `TypedMultipart<T>` rejects with `MultipartError`**
  (`<TypedMultipart<T> as FromRequest<S>>::Rejection` was `Response`). It
  converts into `Rejection` by its variant; hand-written handlers that
  forwarded the `Response` call `.into_response()` (or `Rejection::from`).
- **Breaking: a `Form<T>` body that does not deserialize is `InvalidBody`
  (422)**, no longer `InvalidForm`; the status axum carries (422) is
  unchanged, so `HttpError` responses do not move, but an envelope remapping
  `InvalidForm` no longer sees it. A query-string form (GET/HEAD) stays
  `InvalidForm` (400). `RawForm`: a wrong content type was already
  `UnsupportedMediaType`; the other failures, previously all `InvalidForm`,
  now map to `PayloadTooLarge` (413), `BodyRead` (unreadable body) or the
  kind matching their status.
- **Breaking: OpenAPI documents the panic 500 on every route** through the
  application envelope (`ErrorProjector`, default `HttpError`) — what the
  catch-panic layer answers — merged as an `anyOf` when the route's envelope
  shares the status. Optional identities (`Option<identity>` field or
  parameter) now document 401 (an invalid token still fails), SSE routes
  document their `Query`/`Path`/`Form`/garde failures and WS routes their
  `Query`/`Path`/garde failures (WS extracts request parts only, so `Form<T>`
  does not apply there), and a `Form<T>` body documents 415/413/400/422
  instead of 400.
- **Breaking: unknown routes and wrong methods answer JSON** (task #1072, phase P4). An
  app without a fallback of its own used to get axum's empty-bodied 404 and
  bodiless 405; they are now `404 {"error":"Not found"}` and `405
  {"error":"Method not allowed"}` with `content-type: application/json` (or
  the `error_projection::<E>()` envelope). `CatchPanicLayer::with_hook(hook)`
  is replaced by `CatchPanicLayer::with(hook, projector)` and
  `catch_panic_layer_with` takes the projector too.
- **Breaking: `SchemaGap` gains the `ErrorBodyInlined { component }` variant**
  (r2e-openapi, see *Added*). Exhaustive `match`es on `SchemaGap` need a new
  arm or a fallback.

### Added

- **example-app: custom error envelope demo** — `Problem` (RFC 9457
  `application/problem+json`, `Validation` remapped to 422) in
  `examples/example-app/src/error.rs`, `ProblemController` (`/problems`) showing a
  route envelope next to an infallible route, `.error_projection::<AppError>()`
  installed in `app.rs`, and `tests/http/error_envelope.rs` pinning the bodies
  and the per-route OpenAPI error responses. The book chapter *Error Handling*
  gains a "Rejection, envelopes and projection" section, *OpenAPI* an "Error
  responses" section, and the 0.4 → 0.5 migration guide is linked from the book
  and the README.
- **`Rejection` — one typed value for every framework failure** (task #1072,
  phase P0 of `plans/error-projection.md`). `r2e_core::error::Rejection { kind:
  RejectionKind, status, message, details, headers, source }` is the hub every
  fault converts into with plain `From`: the axum `Json`/`Path`/`Query`/`Form`
  rejections, `ParamError` (by its new `location`), `HttpError`, `GuardError`,
  `MultipartError`, garde reports, `SecurityError` (with `WWW-Authenticate`),
  `TenantError` (`TenantError::into_rejection(TenantStatuses)` keeps the
  configured statuses), a raw `Response` (kind `Opaque`).
  `RejectionKind::default_status()` is the one status table and
  `RejectionKind::from_status` its inverse. `Rejection::project::<E>()` renders
  through an envelope `E: From<Rejection> + IntoHttpResponse + ErrorSchema`;
  `HttpError` is the default envelope and its bodies are byte-equal to 0.4.
- **`ErrorSchema`** (`r2e_core::error`, prelude): the static side of an error
  envelope — `status_of(kind)`, `body_schema()`, `body_schema_for(kind)`,
  `extra_statuses()`, `opaque_passthrough()` — read by the runtime and, from
  phase P2, by the OpenAPI builder.
- **Typed guard errors**: `r2e_security::RolesDenied`,
  `r2e_rate_limit::RateLimited { retry_after }` (emits `Retry-After`, seconds
  rounded up) and `r2e_openfga::FgaDenied`. Each is `Error`, converts into
  `Rejection`, and renders through it; the built-in guards use them and their
  bodies are unchanged.
- **`#[derive(ApiError)]`: `#[error(rejection)]`** on one variant holding a
  `Rejection` (alias: `#[error(transparent)]` over a `Rejection` field). The
  derive then also emits `From<Rejection>` and `ErrorSchema`. An enum with
  exactly one `#[error(transparent)]` variant over `HttpError` inherits both
  impls. `status`/`message` on that variant and two such variants are compile
  errors.
- **One projection point per route** (task #1072, phase P1). Every HTTP
  route, SSE and WS endpoint is now one generated entry fn `(State, Request)
  -> Response` that owns the whole pipeline — pre-auth guards → request data
  (identity + `#[inject(request)]`) → guards → head parameters → body
  parameter (last) → garde validation → managed acquire → interceptors →
  handler → managed finalize — and converts every failure into a `Rejection`
  projected **once** through the route's error envelope. The envelope is
  inferred from the handler's return type: `Result<T, E>` with
  `E: From<Rejection> + IntoHttpResponse + ErrorSchema` projects through `E`
  (no attribute to write); any other return type uses the app-level
  projection. Guards run before the route's own parameters, so a denied or
  unauthenticated request never reads its body. New tests:
  `r2e-core/tests/http/projection.rs`.
- **`AppBuilder::error_projection::<E>()`** — the app-level error envelope
  (the JAX-RS/Quarkus `ExceptionMapper` equivalent), provided as the
  `ErrorProjector` bean (`r2e_core::ErrorProjector`, `of::<E>()` /
  `project(rejection)`). Routes whose return type declares no envelope, SSE
  and WS routes, and the framework's own responses (phase P4: the catch-panic
  500, the router's 404 and 405, the `Json` extractor's 413) render through
  it; without the bean the default is `HttpError`, byte-equal to 0.4.
- **Framework 404/405/panic through the application envelope** (task #1072,
  phase P4). `build_inner` reads the `ErrorProjector` bean once
  (`ErrorProjector::default()` = `HttpError`) and hands it to the two
  catch-panic slots (`CatchPanicLayer::with(hook, projector)`, the 500 being
  `Rejection::internal("Internal server error")` projected) and to the
  router: a framework `fallback` answering `Rejection::not_found("Not found")`
  — installed only when nothing else claimed the fallback, so a controller
  `#[fallback]`, a merged `Router::fallback(..)` and the `r2e-static` SPA
  fallback keep winning — and a `method_not_allowed_fallback` answering the new
  `RejectionKind::MethodNotAllowed` (405; `Allow` kept). `E::status_of` applies
  to all three. New: `r2e_http::routing::has_custom_fallback(&Router)` (reads
  the bit axum only exposes through `Router`'s `Debug` output; pinned by
  `r2e-http/tests/routing.rs`). Tests: `r2e-core/tests/http/fallback.rs`,
  `tests/http/panic.rs`.
- **OpenAPI error responses from the route's real failures** (task #1072,
  phase P2). `RouteInfo` gains `rejection_kinds: Vec<RejectionKind>` (inferred
  by `#[routes]`: body extractor kinds, `Path`/`Query`/`Form`/`#[derive(Params)]`
  locations, garde validation, identity (required or optional), roles/guards, rate-limit
  guards, always `Internal`) and `error_schema: Option<ErrorSchemaInfo>` (the
  envelope of a `Result<T, E>` return type). `build_spec` documents one
  response per distinct `ErrorSchema::status_of(kind)` of the route's envelope
  — the route's own, else the application's `error_projection::<E>()` (read
  from the `ErrorProjector` bean by `OpenApiPlugin`, or set with
  `OpenApiConfig::with_error_schema::<E>()`), else `HttpError` — with the
  envelope's body components and `extra_statuses`; bodies are deduplicated
  by schema and distinct bodies on one status become an `anyOf`; an error
  body whose component name is taken by a different schema (DTO, registry,
  nested `$defs`) is inlined and warned about (`SchemaGap::ErrorBodyInlined`,
  `build_spec_with_warnings`). New `r2e_core::error::ErrorSchemaInfo` (`Copy`
  capture of an `ErrorSchema` impl, also exposed as `ErrorProjector::schema()`)
  and `r2e_core::di::meta::RequestBodySchema` (a custom last-parameter body
  extractor documents its content type, schema and rejection kinds).
- **`RequestData<S>`** (`r2e_core::web::extract`): the R2E-owned trait the
  generated `__R2eRequestData_<C>` extractor implements —
  `extract(&mut Parts, &S) -> Result<Self, Rejection>` — replacing its
  `FromRequestParts` bridge impl.
- **gRPC guards and identity** (task #1072, phase P3). `#[grpc_routes]`
  methods and impl blocks take `#[guard(..)]`, `#[roles(..)]` and
  `#[all_roles(..)]` — the HTTP `Guard<I>` impls, built once at registration
  through `DecoratorSpec` with their bean deps checked at
  `register_grpc_service` — and `#[inject(identity)]` **method parameters**
  (`AuthenticatedUser` or `Option<AuthenticatedUser>`, any position). The
  guard sees a `GuardContext` built by `r2e_grpc::guard_context` (request
  metadata as `headers`, `extensions`, `peer_addr`). Per call: identity →
  controller guards → method guards → interceptors → method.
- **`r2e_grpc::GrpcIdentity`**: how an `#[inject(identity)]` gRPC parameter is
  read — `type Spec: DecoratorSpec` resolves the extractor from the graph at
  registration (so the bean is a compile-time dependency of the service),
  `extract` / `extract_optional` read the metadata per call and return a
  `Rejection`. `r2e-security` (feature `grpc`) implements it for
  `AuthenticatedUser` with `JwtIdentitySpec` (product: the
  `Arc<JwtClaimsValidator>` bean): `authorization: Bearer <jwt>`, validated by
  the same bean HTTP uses. `r2e_grpc::bearer_token` is the typed metadata read.
- **`r2e_grpc::rejection_to_status`** (+ `code_from_status`): projects a
  `Rejection` onto `tonic::Status` by kind — Unauthenticated →
  `UNAUTHENTICATED`, Forbidden → `PERMISSION_DENIED`, NotFound → `NOT_FOUND`,
  Conflict → `ABORTED`, RateLimited/PayloadTooLarge → `RESOURCE_EXHAUSTED`,
  Unavailable → `UNAVAILABLE`, Timeout → `DEADLINE_EXCEEDED`, Internal →
  `INTERNAL`, request-shape kinds → `INVALID_ARGUMENT`, others by HTTP status;
  the message becomes the status message and the rejection's headers
  (`Retry-After`, `WWW-Authenticate`) become response metadata. A free
  function because `From<Rejection> for Status` would be an orphan impl.
- **`McpError: From<Rejection>`**: MCP guard rejections map by kind
  (Unauthenticated → `Unauthorized`, Forbidden → `Forbidden`, NotFound →
  `NotFound`, request-shape kinds → `InvalidParams`, Internal/Unavailable/
  Timeout → `Internal`, Conflict/RateLimited → `Tool` with the details, others
  by status) instead of re-reading a rendered response body.
- Compile-time checks on gRPC services: a `#[roles]` method (or any guard
  whose spec sets `REQUIRES_IDENTITY`) without an `#[inject(identity)]`
  parameter, a struct-level `#[inject(identity)]`, and a missing
  `Arc<JwtClaimsValidator>` bean all fail to build.

- **`StopPhase::AfterDrain` background services** (task #1071). A
  `ServiceComponent` can declare `fn stop_phase() -> StopPhase` and
  `fn stop_order() -> i32` — with the derive,
  `#[service(stop = "after_drain", order = N)]`. Such a service is cancelled
  **after** the HTTP drain and the tracked-handle join, in a new shutdown step 5,
  one service at a time in ascending order (each join bounded by
  `shutdown_grace_period`), so a sink fed by request handlers (write-behind,
  audit/outbox) sees the last requests' output instead of being torn down at
  step 2 while the listener is still serving. Its token hangs off a separate
  post-drain root with its own drop guard, so a dropped `run()` future or a
  dropped `RunningApp` still cancels/aborts it. `order` without
  `stop = "after_drain"` and an unknown `stop` value are compile errors.
- **`on_shutdown_after_drain_async` plugin hook** (`PluginBuildContext` and
  `DeferredContext`): an async cleanup hook awaited at step 5, after the
  after-drain services, for resources that handlers and sinks still use
  during the drain.

### Changed

- `MultipartError` now implements `std::error::Error`.
- **`r2e-executor`: the pool drains after the HTTP drain, not before it**
  (task #1071). The `Executor` plugin's graceful drain moved from
  `on_shutdown_async` (step 2) to `on_shutdown_after_drain_async` (step 5).
  Previously a draining pool rejected every `submit` with
  `RejectedError::Shutdown` while in-flight handlers were still running, so a
  handler — or a sink fed by one — could not hand work to the pool during the
  drain. The shutdown sequence is now six steps; see
  `docs/features/22-serve-lifecycle.md`.

### Fixed

- **gRPC: controller-level and method-level guards get separate
  `GuardContext`s**, as on HTTP and MCP: impl-block guards are built once per
  controller and run with `method_name: "*"`, method guards with the method
  name. A controller-level `RateLimit` on a gRPC service is now one
  service-wide budget instead of being charged twice per call under the
  method's key.
- **Rejection headers merge per name on every path.** `Rejection::project`
  now adds the hub's headers to an opaque passthrough response too (a
  `Retry-After` added to `Rejection::from(response)` used to be dropped), and
  keeps every value of a repeated header (two `WWW-Authenticate` challenges).
  A header name the rendered response already carries still wins.
- **`Result<impl Trait, E>` routes keep their envelope.** The return-type
  probe can't name an opaque success type, so such routes fell back to the
  app-level projection (runtime and OpenAPI); they now project through `E`.
- **`r2e-executor`: an aborted job no longer leaks the pool's counters**
  (task #1066). `JobHandle::abort` drops the job future at its next await, so
  the bookkeeping written after `fut.await` never ran: one aborted running job
  pinned `drain_count` above zero forever and every `shutdown_graceful` after
  it sat out the whole `executor.shutdown-timeout` on an idle pool; an aborted
  queued job leaked its `queued` slot and made `try_submit` reject early. The
  counters are now paired through RAII guards (`QueuedGuard`, `RunningGuard`)
  whose drops land on a return, an unwind and a dropped future alike, and both
  submission paths share one job body (`run_job`).

## [0.4.0] - 2026-09-26

### Changed

- **Releases are version-driven** (see the note above): `release.yml` no longer
  tags every merge; it publishes to crates.io, then tags `vX.Y.Z`, only when a
  `release: X.Y.Z` PR changes the workspace version, and only after Tests
  pass. New `scripts/bump-version.sh`; `publish-crates.sh --yes` for CI.

- **`ErrorHandling` plugin removed** (PR #68/#69, task #1025). Panic capture is
  now part of router assembly, so the plugin was a third copy of the layer that
  never fired for a handler panic. **Breaking**: `.plugin(ErrorHandling)` and
  `catch_panic_layer()` no longer compile — delete the line, nothing replaces it.

- **Plugin routes mount in the Routes stage** (PR #69). Prometheus `/metrics`,
  the `r2e-oidc` endpoints and `EmbeddedFrontend`'s SPA fallback go through
  `after_routes` instead of an `add_layer` closure, so they now sit inside
  `HttpTrace`, the metrics layer and the catch-panic slot (static responses are
  traced and counted; `EmbeddedFrontend` no longer has to be installed last).
  Rule: `add_layer` wraps, it never mounts. **Breaking**: `EmbeddedFrontend`
  combined with a controller `#[fallback]` is now a boot panic instead of a
  silent override.

- **MCP session binding** (PR #74). Sessions are bound to their principal: a
  `Mcp-Session-Id` replayed by another subject gets HTTP 404 (was JSON-RPC
  -32600). **Breaking**: `ToolRoute` / `ResourceRoute` / `PromptRoute` gain
  `group` (plus `completions` on resources and prompts), `McpRoutes` gains
  `uses_session`, `ToolCall` / `ResourceCall` / `PromptCall` gain `session` and
  `progress` (use the new `::new` constructors), `McpServer::Provided` gains
  `McpSessions`, `McpSessionError` gains `InvalidCompletion`. `McpSession`
  under `mcp.stateless` panics at boot.

### Added

- **`attach_reuseport_cbpf` ingress helper** (PR #75): installs a classic-BPF
  steering program on a `SO_REUSEPORT` group (`SO_ATTACH_REUSEPORT_CBPF`,
  Linux) — the program returns the target socket's group index, i.e. its bind
  order — with `CbpfInsn` in the kernel `sock_filter` layout. **Breaking**:
  new `AffinityError::ReuseportFilterUnsupported` variant (returned off Linux).

- **Observable panics and a unified `on_panic` hook** (PR #68/#70/#71, tasks
  #1017/#1027). R2E's own catch-panic layer, installed innermost (so a handler
  panic still gets the `request completed` line, the 5xx metric and
  `x-request-id`) and outermost as a last-resort net, emits one
  `tracing::error!` on target `r2e::panic` with the message and route.
  `AppBuilder::on_panic(|report| ..)` is called once per panic from HTTP
  handlers, `#[scheduled]` ticks (shared or dedicated pool, tick factories
  included) and `PoolExecutor` jobs; `PanicReport::origin()` returns a
  `PanicOrigin::{Http, Scheduled, Executor}` and `label()` a bounded metric
  label. A panicking hook is contained. New `PoolExecutor::submit_named`;
  `#[async_exec]` jobs are named after their method. **Breaking**: hooks that
  assumed HTTP-only now also see background panics.

- **MCP per-session member lists and protocol gaps** (PR #74).
  `#[mcp_routes(group = "…", opt_in)]` groups; an `McpSession` member parameter
  enables/disables groups and adds session-private `DynamicTool` /
  `DynamicResource` / `DynamicPrompt` members (with `list_changed`
  notifications); `McpServer::session_init::<T: McpSessionInit>()` and the
  `McpSessions` bean. Protocol: `Progress` parameter
  (`notifications/progress`), live elicitation via `McpClient<'_>`
  (`elicit::<T>()`, `elicit_url()`, `mcp.elicitation-timeout-secs`),
  `#[completion]` providers wired with `complete(arg = "fn")` and checked at
  compile time, and `*/list` pagination (`mcp.page-size`) with cursors bound
  to the caller's visible list.

- **Per-request span enrichment channel** (task #1015): the `HttpTrace` layer
  now publishes the request span as the `RequestSpan` request extension —
  handlers take it as a parameter and `record(..)` domain fields their
  `MakeRequestSpan` declared `Empty` (`session_id`, `tenant_id`, …), at any
  call depth and without task-local plumbing (excluded routes yield a no-op
  `Span::none()`). A new defaulted `MakeRequestSpan::make_state` allocates an
  optional per-request `SpanState` slot (type-erased `Arc`), published as a
  request extension and handed back to `on_response` — the way values written
  *during* the request reach a custom summary event, since span fields are
  write-only. **Breaking**: `MakeRequestSpan::on_response` gains a
  `state: Option<&SpanState>` parameter (default impl unchanged otherwise).

- **`TestApp` can reuse an `App::Env` across boots** (task #988): three new
  boots skip `A::setup()` and build on an environment the caller already owns —
  `TestApp::boot_env::<A>(env)`, `TestApp::boot_with_env::<A>(env, configure)`,
  `TestApp::boot_plain_env::<A>(env, configure)`, plus `try_boot_env` /
  `try_boot_with_env` / `try_boot_plain_env`. `App::Env` is already
  `Clone + Send + Sync + 'static`, so a test binary builds the expensive part
  once and boots every test off it instead of replaying pools and migrations
  per test (`#[before_all]` only amortises inside one suite).

  ```rust
  use r2e_test::SharedEnv;

  static ENV: SharedEnv<MyApp> = SharedEnv::new();

  #[r2e::test(app = my_app::MyApp, env = ENV.get().await)]
  async fn lists_users(app: TestApp) {
      app.get("/users").send().await.assert_ok();
  }
  ```

  `r2e_test::SharedEnv<A>` is the supported way to memoise it: `new()` /
  `with(init)` are `const` (so they go into a `static`), `get().await` /
  `try_get().await` build the environment **once per process on a runtime
  `r2e-test` owns and never shuts down**, and concurrent first callers share the
  one run. A bare `OnceCell`/`LazyLock` must not be used: `#[r2e::test]` builds
  one runtime per test and drops it at the end of the test, so an environment
  initialised there keeps its value but loses its reactor (listeners, pool
  keep-alive tasks, timers, anything `setup` spawned), and later tests hang on
  an inert environment.

  `#[r2e::test]` and `#[r2e::test_suite]` gained the matching `env = <expr>`
  knob (evaluated inside the test's async block, composes with `with = …` and
  `jwt = false`, requires `app = …`; on a suite it also requires a
  `#[before_all]` that binds the booted app, since that hook is what evaluates
  it — likewise for `with = …` / `jwt = …`). Everything else is unchanged:
  `test` profile, pinned `TestJwt` validators, the production startup phase,
  `shutdown()`. **Isolation is the caller's job** — a shared `Env` is shared
  state across concurrently running tests. The harness never disposes the `Env`
  itself, but `shutdown()` does run whatever `A::build` registered, so an app
  that hands an `Env`-owned resource to a disposer still invalidates it for
  later boots.

- **Feature modules own their gRPC services** (task #989): `#[module]` gained a
  `grpc_services(...)` key, the transport peer of `controllers(...)`. A vertical
  slice now declares its gRPC service next to its HTTP controllers, and that
  service may inject the module's **private** providers — which the app-level
  `.register_grpc_service::<S>()` cannot, since it checks the service's deps
  against the application state (so a module bean had to be exported to be
  injectable).

  ```rust
  #[module(providers(GreetingRepo), grpc_services(GreeterService))]  // GreetingRepo stays private
  pub struct GreetingModule;

  AppBuilder::new()
      .plugin(GrpcServer::on_port("0.0.0.0:50051"))
      .register_module::<GreetingModule>()   // no .register_grpc_service::<_>() needed
      .build_state()
      .await
  ```

  The services are dependency-checked **module-locally** at `register_module`
  (deps ⊆ providers ∪ imports, `#[intercept(...)]` spec deps folded in, exactly
  like `M::Controllers`) and registered by `build_state()` from the module's
  retained `BeanContext`, in declaration order after the module's controllers.
  The key also implies the `GrpcServer` plugin: the macro appends it to the
  module's `RequiredPlugins`, so forgetting `.plugin(GrpcServer::...)` is a
  compile error **naming `GrpcServer`** rather than a service silently
  registered into a registry nobody drains. A module may equally bring the
  plugin itself with `plugins(GrpcServer = GrpcServer::on_port(..))`.
  `RequiredPlugins` is verified by checking the plugin's *provisions* against
  the provision list, so `GrpcMarker` — what `GrpcServer` provides — is now
  unconstructible outside r2e-grpc: nobody can hand-`.provide(..)` it to make a
  module compile without the plugin (and hence without the registry the module
  registers into). A hand-written `impl FeatureModule` that skips
  `RequiredPlugins` altogether still fails at boot, with
  `BeanError::MissingTransportPlugin` naming both the plugin and the module.

  Each service is registered **once**: a name already in the registry — the same
  service in two modules' `grpc_services(..)`, or in a module *and* an app-level
  `.register_grpc_service::<S>()` — fails instead of handing tonic two
  overlapping route sets under one name. The module path reports
  `BeanError::DuplicateEndpoint` on the `try_build_state()` channel; the
  app-level call panics (that call site already panics for a missing plugin);
  a service listed twice in one `grpc_services(..)` is a macro compile error.

  r2e-core stays transport-agnostic: `FeatureModule` gained
  `type Endpoints: ModuleEndpointSet` (type-level `Deps` only) and the
  value-level `ModuleEndpoints<T>` registration hook, both implemented in
  r2e-grpc for the new `r2e_grpc::ModuleGrpcServices<(A, B)>` (what the macro
  generates). Modules without `grpc_services(..)` emit `type Endpoints = ();`
  and no r2e-grpc path, so they still compile in apps without the gRPC feature.

  **BREAKING (pre-production, no compatibility shim)**:
  - hand-written `impl FeatureModule` blocks must add `type Endpoints = ();`
    (stable Rust has no associated-type defaults) — the `#[module]` macro
    generates it;
  - `RegisterModule` gained a witness type parameter (`EndpIdx`) — only visible
    to code that names the trait's parameters explicitly;
  - three new `BeanError` variants, `EndpointConfig`, `MissingTransportPlugin`
    and `DuplicateEndpoint` — an exhaustive `match` on `BeanError` must add arms;
  - `GrpcServiceRegistry::add_service` returns `Result<(), DuplicateService>`
    (it was `()`), and `GrpcMarker` is no longer constructible outside
    r2e-grpc;
  - registering the same gRPC service twice now fails (module: boot error;
    app-level `.register_grpc_service::<S>()`: panic) instead of silently
    double-registering it.

- **`rt::RuntimeId`** (`Runtime::id()` / `RuntimeHandle::id()`): the identity of
  a runtime, comparable across threads, for asserting that two pieces of work
  share one reactor. Used by `#[r2e::test_suite]`'s guard-rail (see Fixed).
  Ids are unique only among *live* runtimes: once a runtime is dropped its id
  may be reused by a later, unrelated one, so an id only proves shared identity
  when compared against a runtime known to be alive (which is how the suite
  guard uses it — the suite owns the runtime it names).

- **`rt::Runtime::shutdown_timeout` / `shutdown_background`**: shut a runtime
  down explicitly, for runtimes parked in a `static` that can never be dropped
  by going out of scope. `#[r2e::test_suite]` uses the first one at teardown.

- **Worker scopes and verifiable multi-worker serving** (task #990, ADR
  `docs/adr/0001-worker-scopes-and-planes.md`): `WorkerInfo` (stable worker
  identity — id / count / role / effective CPU — readable anywhere, incl. as a
  handler parameter), `WorkerLocal<T>` + `AppBuilder::worker_local` (exactly one
  `!Send`-capable `T` per worker, built/used/dropped on its worker thread),
  `WorkerSet` + `WorkerState` + `WorkerHealth` (aggregated per-worker lifecycle
  and errors, health indicator), `Mailboxes<M>` (counted cross-worker messaging
  with `send_to`/`broadcast`/`ask_all`), `r2e::runtime::ingress`
  (`reuseport_tcp`/`reuseport_udp`/`adopt_*`, `AffinityError::Unsupported` — no
  silent fallback), `WorkerCollector` in `r2e-prometheus` (`r2e_worker_*`
  series), and `WorkerHarness` for deterministic tests. Docs in
  `docs/features/19-sharded-serving.md`; example `examples/example-worker-udp`
  rewritten as a shared-nothing service with control-plane aggregation.

- **grpc-web on the multiplexed gRPC transport** (feature `grpc-web`, `web` on
  `r2e-grpc`): `GrpcServer::multiplexed().with_grpc_web()` (or
  `.with_grpc_web_cors(CorsLayer)`) adds a `tonic-web` arm to
  `MultiplexService` for `application/grpc-web`, `grpc-web+proto` and
  `grpc-web-text` requests over HTTP/1.1 and HTTP/2, with CORS preflight
  handling. Without it grpc-web requests still get `415` + a boot warning.

- **`r2e::http::IntoHttpResponse`** — R2E's own response-conversion contract,
  the counterpart of `FromRequestPartsVia` on the extract side. R2E error types
  (`HttpError`, `ParamError`, `MultipartError`, `RequestId`, `SecurityError`,
  `TenantError`, `OidcError`) and everything `#[derive(ApiError)]` generates now
  implement **this** trait instead of the HTTP backend's `IntoResponse`, and
  bridge to the backend through a single macro:

  ```rust
  impl IntoHttpResponse for MyError {
      fn into_http_response(self) -> Response { /* … */ }
  }
  r2e::http::impl_into_response!(MyError);
  ```

  **Not a break**: the bridge emits the backend impl, so every type that was
  returnable from a handler still is, and `Result<T, E>` / `(StatusCode, T)`
  composition is unchanged. A hand-written `impl IntoResponse for MyError` also
  keeps working — `IntoHttpResponse` is the recommended way, not the only one.
  The macro is a macro rather than a blanket impl because
  `impl<T: IntoHttpResponse> IntoResponse for T` is an orphan impl and the
  mirror blanket would forbid all per-type impls; see `r2e-http/src/response.rs`.
  `IntoHttpResponse` is in the prelude.

- **`r2e::http::axum_compat`** — the explicit escape hatch to the raw `axum`
  API (`use r2e::http::axum_compat::axum;`), for the cases a re-export shim
  cannot cover: tower layers with axum-typed bounds, `axum::debug_handler`,
  third-party crates whose API is spelled in axum types. This settles §5.3d of
  `plans/runtime-http-dependency-containment.md` as **decision A**: R2E's public
  promise is *R2E types* under `r2e::http` / `r2e::prelude` plus R2E's own
  contracts (`IntoHttpResponse`, `FromRequestPartsVia`); axum stays reachable,
  but only through a name you have to type on purpose. Apps should still not
  add `axum` to their own `Cargo.toml`.

- **New crate `r2e-rt`** — the async-runtime facade, sitting at the **bottom**
  of the workspace dependency graph (below `r2e-http`). It is now the single
  workspace member allowed to name `tokio` / `tokio-util` / `tokio-stream`
  directly, so swapping the runtime — or moving further towards thread-per-core
  sharded runtimes — is a change in one crate instead of a hunt across dozens of
  call sites. Two enforcement scripts freeze the boundary
  (`scripts/check-dep-boundary.sh`, `scripts/check-source-boundary.sh`).
  `r2e-core/src/rt.rs` moved into it wholesale and `r2e_core::rt` is now a
  re-export, so **`r2e::rt::…` / `r2e_core::rt::…` keep resolving to exactly
  what they always did** (`spawn`, `spawn_ctl`, `spawn_blocking`, `JobHandle`,
  `sleep`, `timeout`, `interval`, `bind_tcp`, `shutdown_signal`, …).
  New in the facade, on top of the moved surface:
  - `rt::CancelToken` / `rt::CancelDropGuard` — wrappers over
    `tokio_util::sync::CancellationToken` / `DropGuard`, so an app can consume
    R2E's shutdown API without adding `tokio-util` to its own `Cargo.toml`.
    `From` conversions both ways keep the not-yet-migrated crates working.
  - `rt::sync` — re-exports of `mpsc`, `oneshot`, `broadcast`, `watch`,
    `Mutex`, `RwLock`, `Notify`, `Semaphore`, `OnceCell`.
  - `rt::{select!, pin!, join!}`, `rt::JoinSet`, `rt::stream`,
    `rt::{RuntimeBuilder, Runtime, block_on}`.
  - `rt::Instant` + `rt::sleep_until(deadline)` — the deadline form of
    `rt::sleep`, on the runtime's own monotonic clock; what a timer wheel driven
    by absolute fire times needs (the scheduler's min-heap driver).
  - `rt::yield_now()` and `rt::in_runtime()` — the latter is the non-panicking
    probe behind `current_handle`, for synchronous paths that may run outside a
    runtime (a `Drop` impl detaching cleanup work).
  - A non-default `test-util` feature (`tokio/test-util`), off by default
    because paused clocks must not reach the whole workspace through feature
    unification.
  - `rt::TcpStream` and the `rt::io` module (`AsyncRead` / `AsyncWrite` and
    their `…Ext` traits, `BufReader`, `BufWriter`, `duplex`) — re-exports, the
    same treatment as `rt::TcpListener` and `rt::sync`. They are what raw-socket
    test code and byte-stream plumbing need, and their absence was the last
    reason to keep a direct `tokio` dependency around. `rt::stream::wrappers`
    also carries `TcpListenerStream` now (tokio-stream's `net` feature).

### Fixed

- **`#[derive(ConfigProperties)]` garde rules now run** (PR #73, task #1046).
  The derive looked for `#[validate(..)]` instead of `#[garde(..)]`, so
  validation was never emitted. It now runs after construction and reports
  `ConfigError::Validation` with dotted keys (`app.pool.size`). **Behavior
  change**: a violated rule now fails `from_config`, and therefore boot.

- **Executor drain no longer stalls on a panicking job** (PR #70): panics are
  caught at poll level, so permit release and drain/completed counts survive
  and `shutdown_graceful` completes.

- **`r2e::prelude` no longer ambiguous with both data backends enabled**
  (task #1016). The prelude glob-re-exported `r2e_data_sqlx::prelude::*` and
  `r2e_data_diesel::prelude::*` unconditionally, and the two backends
  deliberately export mirrored names (`DbPool`, `DbTx`, `Tx`,
  `DataSourceHealth`, `TenantPools`, `TenantTx`) — so any build enabling both
  `data-sqlx` and `data-diesel` (a dual-backend app, or a workspace sibling
  pulling the other backend through cargo feature unification) made every use
  of a mirrored name a deny-by-default `ambiguous_glob_imports` error. With
  exactly one backend enabled its prelude joins `r2e::prelude` as before; with
  both, only the backend-unique names (`SqlxDataSource`, `SqlxTx`,
  `DieselDataSource`, `DieselTx`) remain and the mirrored ones are imported
  explicitly from `r2e::r2e_data_sqlx` / `r2e::r2e_data_diesel` (an explicit
  import shadows the glob, so it is stable under both modes).

- **`#[sse]` / `#[ws]` routes publish their parameters in the OpenAPI spec**
  (task #1013, follow-up of #1009). Streaming metadata hardcoded
  `params: vec![]`, so a `#[derive(Params)]` argument — which the generated
  handler really does extract from the request — never appeared in
  `/openapi.json`, and neither did a `Path<T>` argument. Both route kinds now
  build their `RouteInfo.params` through the same code path as a verb route
  (`Path(name): Path<T>` literals + the `ParamsMetadata` autoref probe, then
  deduplicated), so moving a documented `#[get]` to `#[sse]` keeps its
  parameters as well as its prose. A WebSocket method's `WsStream`/`WebSocket`
  argument comes from the upgrade rather than an extractor and is excluded.

- **`r2e-core`'s `runtime` test target is green under `--features dev-reload`
  again** (task #995). Two independent causes, neither of which CI saw (no
  workflow runs the `dev-reload` feature). (1) The builder-level per-worker
  service test served a sharded app, but `dev-reload` deliberately forces the
  single cached-listener path, so `run()` rejects the registration by design;
  the test is now compiled out under the feature and replaced by one that
  asserts the rejection. (2) The dev-reload hot-patch tests shared a process
  with the ordinary serving tests. `mark_hot_reload_loop()` is process-global
  and one-way, so once a dev test had armed it the next served app set
  `LIFECYCLE_INITIALIZED` — after which *every* later `run()` in the binary
  skipped consumers, serve hooks and startup hooks and quietly lost its
  `spawn_service` tasks (`shutdown_budget::grace_period_bounds_a_stubborn_service_and_names_it`
  was the visible casualty). No lock can fix that across parallel test threads,
  so the dev-reload tests now live in their own target,
  `r2e-core/tests/dev_reload/`, and the `runtime` target no longer needs the
  `dev_serial` lock at all.

- **The `dev-reload` per-worker-service error no longer gives impossible
  advice.** It used to be built from `PER_WORKER_REQUIRES_SHARDING_MSG`, so it
  told you to set `server.workers` — a key `dev-reload` ignores. It now states
  that the feature forces single-listener serving and that per-worker services
  require a build without `dev-reload` (and a platform with SO_REUSEPORT
  sharding — dropping the feature is necessary, not sufficient).

- **Attribute macros no longer drop the attributes you write** (task #985).
  Several attribute macros rebuild the item they annotate from its pieces
  (visibility + signature parts + body) so they can strip R2E's own parameter
  and field attributes. Those rebuilds silently discarded everything else.
  - `#[producer]` dropped the whole `attrs` list of the annotated function:
    `#[allow]`/`#[deny]`, `#[inline]`, `#[deprecated]`, `#[must_use]` and doc
    comments written on a producer did nothing. It also dropped `const` and
    `extern "…"` from the signature. All of them are forwarded now, and the
    generated bean struct carries a doc comment of its own so
    `#![deny(missing_docs)]` crates keep building. `#[deprecated]` warns at a
    direct call to the function; the generated struct is a separate item and is
    not itself deprecated, so `.register::<CreatePool>()` stays quiet.
  - `#[routes]` dropped the attributes on the `impl` block (a `#[allow(...)]`
    or doc comment above `impl MyController` vanished) and dropped every
    associated item that was neither a route, a `#[consumer]`, a `#[scheduled]`
    nor a lifecycle hook — an associated `const`, an associated `type` or a
    plain helper `fn` written in a `#[routes]` block disappeared from the
    build. Impl attributes now reach both synthesized impls, and the other
    items stay on the controller core. Note that a route body's `Self` is the
    request façade, so reach an associated const through the controller name
    (`MyController::PAGE_SIZE`). Because there are *two* synthesized impls,
    only **inert** attributes may sit below `#[routes]` — doc comments,
    `#[allow]`/`#[warn]`/`#[deny]`/`#[expect]`/`#[forbid]`, `#[deprecated]`,
    `#[cfg]`, `#[cfg_attr]` and tool attributes (`#[rustfmt::skip]`). Anything
    else (an attribute macro) would expand once per impl, so it is a compile
    error pointing at the position where it runs exactly once: above
    `#[routes]`.
  - `#[bean]` dropped the attributes and the `const`/`extern` pieces of the
    constructor it re-emits.
  - `#[async_exec]` dropped parameter attributes (`#[cfg]` on a parameter,
    `#[allow]`) when re-emitting the wrapper's parameter list. A parameter
    `#[cfg]` is now forwarded to the *forwarding call* as well, so a gated-out
    parameter disappears from the signature and the call together instead of
    leaving the disabled build with an unbound argument.
  - `#[controller]` projects a request-scoped field's attributes onto the
    generated request extractor and façade, and the generated code that binds
    them carries `#[allow(deprecated, non_snake_case)]`: a `#[deprecated]`
    request field warns where *you* read it, not from inside framework code, so
    a crate under `#![deny(deprecated)]` still builds.

  Because rustc evaluates an item-level `#[cfg]` (and a `#[cfg_attr]` expanding
  to one) *before* it invokes an attribute macro — in either attribute order —
  a `#[cfg]`'d-out producer, controller, `#[routes]` impl or bean never reaches
  the macro at all, and no generated impl is left dangling. That is pinned by
  tests rather than assumed (`r2e-core/tests/di/producer_attrs.rs` and
  `r2e-core/tests/controller/attrs.rs`).

  One signature piece is **rejected** rather than forwarded (breaking, but no
  such code compiled before either): an `unsafe fn` `#[producer]` or `#[bean]`
  constructor. R2E generates a *safe* `Producer::produce` / `Bean::build` that
  is the only caller, and the bean graph cannot discharge an `unsafe` contract
  it knows nothing about — re-emitting the signature verbatim is an E0133, and
  adding an `unsafe { }` block around the generated call would sign the
  contract on the user's behalf. Drop `unsafe` from the signature and keep the
  `unsafe { }` block, with its SAFETY comment, inside the body.

- **`#[producer]` now emits `#[allow(clippy::too_many_arguments)]`** on the
  function and on the generated `Producer` impl. A producer takes one parameter
  per dependency, so clippy's 7-argument threshold fires on perfectly
  idiomatic producers and (before the fix above) could not even be silenced.
  User attributes are emitted after it, so `#[warn(clippy::too_many_arguments)]`
  on the function opts back in.

- **`#[r2e::test_suite]` now builds ONE runtime per suite, not one per `#[case]`**
  (task #986). The suite value lives in a module-level `OnceLock` that outlives
  every case, but each generated `#[test]` used to build — and then drop — its
  own runtime. Anything `#[before_all]` amortised that is bound to a reactor (a
  `TestApp`, a `sqlx` pool, a socket, a spawned task, a timer) went inert after
  case 1; because such a resource stops waking rather than erroring, the suite
  failed far from the cause, typically as `PoolTimedOut`. The runtime is now
  owned by `SuiteCell` in that same `OnceLock` and is never dropped, so
  `#[before_all]`, `#[before_each]`, every case, `#[after_each]` and
  `#[after_all]` share one reactor. `#[case(order = N)]` and the per-case libtest
  `#[test]` are unchanged; the runtime knobs (`flavor`, `worker_threads`,
  `start_paused`, …) stay on `#[r2e::test_suite(...)]` and now configure that
  single runtime — note `start_paused` means one paused clock for the whole
  suite instead of a fresh one per case. Guard-rail: every phase
  (`#[before_all]`, each case, `#[after_each]`, `#[after_all]`) asserts from
  inside its `block_on` that it is on the suite runtime and panics naming both
  runtimes if not.

  Teardown: the last case to finish runs `#[after_all]`, then drops the suite
  value *inside* the runtime (so a socket or pool still has its driver in
  `Drop`) and shuts the runtime down with a one-second grace for blocking work.
  Without that the suite's worker threads and detached tasks would outlive it
  for the rest of the test process, since the `OnceLock` is never dropped.
  Anything reaching the suite after teardown panics by name instead of hanging.

  "Last case" is counted against the number of generated `#[case]`s, because
  libtest does not expose which tests the process actually selected. So a
  filtered run (`cargo test some_case`) runs `#[before_all]` and the case but
  never `#[after_all]` — the suite value is leaked to process exit, as before.
  For the same reason `#[ignore]` on a `#[case]` is now a **compile error**:
  it would either suppress teardown entirely or let teardown fire before the
  ignored case runs. Skip inside the case body instead.

  Runtime knobs that make the builder *panic* rather than return an error
  (`start_paused` without `flavor = "current_thread"`, a zero `worker_threads`
  / `max_blocking_threads` / `global_queue_interval` / `event_interval`, a blank
  `thread_name`) are now rejected at macro expansion with a spanned compile
  error, on `#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` alike; any
  remaining builder panic is caught and re-raised naming the suite or test that
  asked for it.

### Changed

- **`r2e-openfga` generates its own gRPC client** — `vendor/openfga-rs` and the
  workspace `[patch.crates-io]` that reached it are gone. A `[patch]` is
  workspace-local and never travels with a published crate, which is what kept
  `r2e-openfga`, `r2e-openfga-macros` and `r2e-openfga-model` off crates.io and
  forced the facade's `openfga` feature to stay commented out; all four are
  released again. The protos live in `r2e-openfga/proto/` and the client is
  generated by `r2e-openfga/codegen` into `r2e-openfga/src/proto/openfga.v1.rs`,
  which is **committed**: a `build.rs` would make `protoc` a hard build
  requirement for every consumer enabling the feature, even though such a
  consumer never authors a proto. `scripts/generate-openfga-proto.sh` is the only
  thing that needs protoc, and CI runs it with `--check` so the file cannot drift
  from the schema. Client only — R2E consumes OpenFGA, it never serves it.

  **Breaking**: `r2e_openfga::openfga_rs` is removed. The wire types and the gRPC
  client are `r2e_openfga::proto::*`, with `tonic` and `prost_types` re-exported
  from the crate root so a caller building a raw request needs no extra manifest
  entry. The derived serde on the wire types goes with it: prost tags oneof
  variants by Rust variant name, which never matched OpenFGA's JSON, so
  `model_convert.rs` was already converting the AST by hand.

- **Release tags now follow the workspace version** (task #1011). The release
  workflow no longer bumps the patch of whatever the latest tag was: it reads
  `version` from the root `Cargo.toml`, tags in that `vX.Y.*` series (patch =
  release counter), and refuses member crates that don't use
  `version.workspace = true`. First aligned tag: `v0.3.0`. The tag ↔ version
  correspondence for the pre-alignment series is documented at the top of this
  file.

- **Perf: one `AuthenticatedUser` per authenticated MCP request** (task #993).
  The auth layer deposited the caller twice — standalone for the identity
  extractor, and inside `McpPrincipal` — so every request deep-copied the
  claims tree, flattened `extra` map included, and the cost grew with whatever
  the IdP put in the token. **Breaking:** `McpPrincipal.user` is now an
  `Arc<AuthenticatedUser>`. Reads are unchanged (`principal.user.sub` goes
  through `Deref`); an owned copy is `(*principal.user).clone()`. The layer
  deposits `Arc::clone(&principal.user)` as the identity extension, and the
  `#[tool]`/`#[resource]`/`#[prompt]` codegen now resolves an identity
  parameter through the new `ToolCall::identity::<T>()` (also on `ResourceCall`
  / `PromptCall`), which looks for `Arc<T>` first and materializes the owned
  `T` only for a member that actually declares one — a member taking
  `ToolCall` can read `call.extension::<Arc<AuthenticatedUser>>()` and copy
  nothing. An authenticated `tools/call` with a 32 KiB claims tree went from
  764 allocations / 129,176 B per request to **200 / 32,976 B** — the same
  cost as a caller with no extra claims at all — and `McpPrincipal::clone`
  (layer, opaque-token cache, `check_access`) from 282 allocations / 47,300 B
  to **0**. Guarded by `r2e-mcp/tests/hotpath/principal.rs` and
  `r2e-mcp/tests/auth/identity.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **Perf: MCP `tools/list` no longer re-allocates the tool metadata**
  (task #994). Every `tools/list` clones the wire payload built at boot (and
  every `tools/call` clones one element of it), but rmcp's `Tool` stores
  `name`/`description` as `Cow<'static, str>` while `ToolRoute::description`
  was an `Option<String>` — so each clone re-allocated a string `#[tool]` had
  emitted as a literal. `ToolRoute::description` is now
  `Option<Cow<'static, str>>` and the macro emits `Cow::Borrowed`: a six-tool
  `tools/list` clone went from 7 allocations / 1080 B to **1 allocation /
  1056 B** (the destination `Vec` alone), and stays there with long
  descriptions where the old path cost 2748 B. **Breaking** for hand-built
  `ToolRoute`s only: `description: Some(s.into())` where `s: String`,
  `Some("…".into())` for a literal — the macro path and the wire format are
  unchanged (pinned by `r2e-mcp/tests/server/wire_golden.rs`). `Resource`,
  `ResourceTemplate` and `Prompt` are `String`-typed in rmcp itself, so their
  lists still copy their strings; both R2E-side halves (built once at boot,
  visibility filter applied before cloning) were already in place. Guarded by
  `r2e-mcp/tests/hotpath/lists.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **Perf: the router state is one `Arc`** (task #992). The HTTP backend clones
  the router state on *every* request, whether or not a handler asks for it, so
  installing the resolved bean HList directly meant one bean `Clone` per bean
  per request — O(N) in the width of the graph, and a deep copy for any bean
  that owns its data. `build_state()` now wraps the materialized list in the new
  `r2e::BeanState<L>` (the list behind a single `Arc`), so the per-request state
  clone costs O(1) at any graph size — the backend takes two state clones per
  request, and each is now a refcount bump rather than one clone per bean:
  measured on a 64-bean state, 128 bean clones per request → 0, and for beans
  owning a `String`, 142 allocations / 10389 B per request → 14 / 1053 B (the
  same as at 8 beans). `state.get::<T>()`, `state.bean::<T>()`, `HasBean` index
  witnesses, `Contains`/`AllSatisfied` bounds and `FromRequestPartsVia` are all
  forwarded through the wrapper, each keeping the cost it already had —
  `get` a fixed-offset field read (now behind one dereference), `bean` the
  same runtime `TypeId` walk it always was — and no application code,
  controller, plugin or macro changes. **Mildly breaking**:
  the state type is now `BeanState<HCons<…>>`, so code that spells the state
  type out (a hand-written `AppBuilder<HCons<A, HNil>>` annotation, a
  hand-assembled test state) must wrap it — `BeanState::new(list)`. Guarded by
  `examples/example-app/tests/hotpath/state.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **BREAKING (`r2e-macros`)**: `#[cfg]` / `#[cfg_attr]` on a **request-scoped**
  controller field (`#[inject(identity)]` / `#[inject(request)]`) is now a
  compile error instead of a silent no-op (task #985). Those fields are
  projected into a positional marker tuple on the generated request extractor,
  which cannot be gated element-wise; conditionally compiling one used to
  produce a mismatched extractor rather than the field the author asked for.
  `#[cfg]` the whole controller instead. App-scoped `#[inject]` / `#[config]`
  fields are unaffected.

- **BREAKING (`r2e-macros`)**: a plain `#[r2e::test]` with parameters is now a
  compile error naming `#[r2e::test(app = MyApp)]` (task #985). Parameters are
  bound from the booted `TestApp`; without an `app = …` there is nothing to
  bind them from, and the generated `#[test]` fn used to fail with a confusing
  libtest signature error.

- **Perf (no API change)**: constant error bodies are no longer built through
  `serde_json::json!` on every response. `SecurityError` (401/503), the panic
  handler's 500, and the rate limiter's 429 / 401 now return a pre-serialized
  `&'static str` body via the new `r2e::http::response::static_json(status,
  body)` helper — `Bytes::from_static`, so no `Value` map allocation and no
  serializer pass per rejection. This is the hot path under unauthenticated or
  throttled traffic. Response bodies are byte-identical. Dynamic messages
  (`ParamError`, `MultipartError`, `HttpError::from_status`) keep going through
  `Json`/`json!`, which escapes interpolated values correctly.

- **BREAKING (`r2e-core`)**: the shutdown-token surface now hands out
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken` —
  `ServeContext::shutdown_token()`, `ConfigWatchContext::{new, shutdown_token}`
  and `LiveConfigReceiver::drive`. Call sites that only `select!` on the token
  or pass it along are unaffected; a site that needs the raw tokio-util token
  (tonic's `cancelled_owned()`, say) converts with `.into()` / `.into_inner()`.

- **BREAKING (`r2e-events`, `r2e-scheduler`)**: the same flip reaches the
  event-bus and scheduler surfaces, which now speak `r2e::rt::CancelToken`:
  `BackendState::{poller_cancels, register_poller_cancel}` and
  `reconnect_loop(…, cancel: &CancelToken, …)` in `r2e-events`;
  `SchedulerHandle::{new, channel, token}`, `jobs_driver`, `start_jobs` and the
  `CancelToken` **bean** the `Scheduler` plugin provides (an app injecting the
  scheduler token writes `#[inject] cancel: CancelToken` now) in
  `r2e-scheduler`. `From` converts both ways with
  `tokio_util::sync::CancellationToken`, so a call site that needs the raw token
  adds `.into()`.

- **BREAKING (`r2e-core`)**: `ServiceComponent::start` now takes
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken`.
  Hand-written background services update their signature (`async fn start(self,
  shutdown: CancelToken)`); `#[derive(BackgroundService)]` users update the
  `run` method it delegates to (`async fn run(&self, shutdown: CancelToken)`).
  With that flip `r2e-tenant`, `r2e-data-sqlx` and `r2e-data-diesel` dropped
  their last `tokio-util` dependency.

- **`r2e-core` no longer depends on `tokio` / `tokio-util` / `tokio-stream` at
  all** (dev-dependencies aside): every internal call site — the builder and
  prepared-server paths, sharded serving, lazy-bean resolution, live-config
  watching, health, SSE/WS, dev-reload — goes through `r2e_core::rt`. Sharded
  serving in particular is now expressible on the facade thanks to two
  additions: `rt::RuntimeHandle` (a wrapper over `tokio::runtime::Handle`, now
  the type of `rt::current_handle`, `rt::control_plane_handle`,
  `rt::set_control_plane` and `Runtime::handle`) and `rt::TcpListener`
  (re-exported, since axum's `serve` takes the concrete type). Also new:
  `rt::block_in_place` and `CancelToken::cancelled_owned`.

- **`#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` and
  `#[derive(BackgroundService)]` now emit facade paths** — the runtime is built
  through `<crate root>::rt::RuntimeBuilder` and the service token is
  `<crate root>::rt::CancelToken`, resolved through the same `r2e` /
  `r2e_core` root every other emitted path uses. **A generated project no
  longer needs `tokio` in its `Cargo.toml`** (`r2e new` stopped emitting it).
  `start_paused = true` needs the paused clock, now behind a forwarded feature:
  `r2e/test-util` → `r2e-core/test-util` → `r2e-rt/test-util`, which `r2e-test`
  turns on so it is present in any crate's dev graph and absent from release
  builds.

- **`clippy.toml`** grew a `disallowed-types` list —
  `tokio_util::sync::CancellationToken`, `tokio::task::JoinHandle`,
  `tokio::runtime::Handle` — next to the existing `disallowed-methods` deny on
  raw spawns. Runtime-neutral primitives (`tokio::sync::*`, `Instant`,
  `JoinSet`, …) stay allowed: they are re-exported by identity. The only
  exemptions are the `#[expect]`-marked wrapper definitions in `r2e-rt`.

- `r2e-events` (+ the `iggy` / `kafka` / `pulsar` / `rabbitmq` backends),
  `r2e-scheduler`, `r2e-executor` and `r2e-tenant` now go through the `rt`
  facade for spawning, timers, sync primitives and `select!`, and **dropped
  their direct `tokio` / `tokio-util` / `tokio-stream` dependencies**. No
  behaviour change; the four distributed backends needed no client-API escape
  hatch.

- **`r2e-http` re-sources the neutral HTTP types from the `http` crate** —
  `StatusCode`, `HeaderMap`, `HeaderName`, `HeaderValue`, `Method`, `Uri`,
  `Parts` and the header constants now come from `http::…` instead of
  `axum::http::…`, and `Extensions` / `Uri` likewise. **No type changes**: axum
  re-exports those very types from `http`, and the workspace resolves a single
  `http` version, so this is identity-preserving for every downstream signature
  — it only stops the workspace from calling `http` types "axum types". The
  `axum::` source baseline drops from 18 files / 32 occurrences to 9 files / 14
  occurrences, all inside `r2e-http/src/` (plan §5 step 3a). Steps 3b (R2E-owned
  `FromParts` / `IntoHttpResponse` traits) and 3c (a `Router` newtype) are
  deliberately **not** done — they are gated on the §5.3d decision about what
  users are promised.

- **The 11 example crates dropped their direct `tokio` / `tokio-util` /
  `tokio-stream` dependencies** and go through the facade like the framework
  does (`rt::sync::*`, `rt::sleep`/`rt::timeout`, `rt::select!`,
  `rt::TcpListener`/`rt::TcpStream`, `rt::io`, `rt::stream`, `#[r2e::test]`).
  With that the tokio dependency allowlist is exactly `{r2e-rt, r2e-test,
  r2e-devservices}` — the by-design set — and the tokio *source* baseline is
  empty workspace-wide.

- **r2e-observability**: `traced_reqwest_client` / `TraceContextMiddleware`
  now open an OpenTelemetry **client** span per outgoing request
  (`otel.kind = "client"`, name `HTTP {method}`, HTTP-client semantic
  conventions: `http.request.method`, `server.address`, `server.port`,
  `url.full`, `http.response.status_code`, `otel.status_code` /
  `error.message`) and propagate **that span's** context instead of the
  caller's. Tracing backends that derive a service graph from CLIENT→SERVER
  pairs (Tempo metrics-generator, Jaeger, Grafana) now show `caller → callee`
  edges and client-side latency for R2E services calling each other.
  Implemented on `reqwest-tracing` pinned to the workspace
  `opentelemetry 0.32` / `tracing-opentelemetry 0.33`. New re-exports:
  `R2eSpanBackend`, `OtelName`, `OtelPathNames`, `DisableOtelPropagation`.
  `inject_current_context` is unchanged (headers only, no client span).
  Follow-up of the outgoing-propagation work (#764, #765, #766); task #927.

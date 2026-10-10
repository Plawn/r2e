# Migration: error projection layers (0.4 → 0.5)

R2E is not in production, so this break (task #1072) shipped without a feature
flag. Every failure the framework raises before, around or instead of a handler —
extractor rejection, `#[inject(request)]` extraction, identity, guard, garde
validation, managed `acquire`/`finalize`, panic, 404/405/413 — is now **one typed
value, `Rejection`**, projected **once per route** into an error envelope `E`
and only then rendered. Layers connect through plain `From`/`Into`; one small
trait, `ErrorSchema`, carries the static OpenAPI metadata, and runtime and spec
read the same `status_of` table. That keeps them in step for **conforming
envelopes** — a `From<Rejection>` impl that renders `rejection.status` rather
than a status of its own — and for the documented kind inference. Known limits:

- the panic 500 is always the **application** envelope's body (the catch-panic
  layer renders through the `ErrorProjector`), whatever the route returns;
- `Internal` stands for the failures the macro cannot enumerate (`#[managed]`
  acquire/finalize, `#[inject(request)]` extractors, an envelope/extractor
  mismatch), so its 500 is documented on every route whether or not it can
  happen;
- an error body whose component name is already taken by a different schema
  (a DTO, a registry entry, a nested `$defs` type) is documented **inline**,
  with a boot warning, instead of as a `$ref`.

`HttpError` is the default envelope and **its own bodies** (`Err(HttpError::…)`
returned by a handler, guard or managed resource) are byte-equal to 0.4. The wire
is **not** unchanged for framework rejections, though:

- **Every extractor rejection** — `Json`/`Path`/`Query`/`Form`/header/multipart
  failures, a missing or wrong `content-type`, a 413 body limit — now renders as
  the envelope's JSON (`{"error": "..."}` with the default `HttpError`) where 0.4
  returned axum's `text/plain` body. Status codes are unchanged.
- The framework's own 404/405 now answer JSON (they had no body in 0.4).

If you never implemented a guard, a `ManagedResource`, a gRPC guard or an MCP
guard adapter, never set `server.params-rejection-format`, and never matched or
constructed `HttpError::Validation(v)`, most code rebuilds unchanged (that
variant is now `HttpError::Validation { status, response }` — build it with
`HttpError::validation(response)` and match the struct form). The breaking-change
table in this guide is the authoritative list of source-level breaks: code that
builds a `ParamError` literal (new `location` field), implements
`PrefixedExtract`, or matches the `#[derive(Params)]` / `TypedMultipart`
rejection types must also be adjusted. Clients that parsed the `text/plain`
rejection bodies or relied on empty 404/405 bodies must be updated.

Design record: `plans/error-projection.md`. User guide: `llm/error-handling.md`.

## What changed

| Old (0.4) | New (0.5) |
|---|---|
| `Guard<I>::check` / `PreAuthGuard::check` → `Result<(), Response>` (or `Result<(), GuardError>` rendered as a response) | `async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Rejection>`; `GuardError`, `RolesDenied`, `RateLimited`, `FgaDenied`, `HttpError` all convert with `?`/`.into()` |
| `ManagedResource::Error: Into<Response>` | `type Error: Into<Rejection>`. `ManagedErr<E>` is a `?`-friendly newtype that still requires `E: Into<Rejection>` — it does **not** make a foreign error convertible; map such errors to `HttpError` (or your envelope) first. `ManagedErr<HttpError>` is the usual choice |
| `ParamsRejectionFormat` + `server.params-rejection-format` config key | removed — rejections are typed (`ParamError { location: ParamLocation, .. }` → `RejectionKind::InvalidPath` / `InvalidQuery` / `InvalidHeader` / … by location) and render through the route's envelope |
| `Via<T, M>` extractor adapter in handler signatures / custom code | removed — the generated entry fn resolves `FromRequestPartsVia` inline; the `ViaAxum` marker stays and now requires the rejection to be `Into<Rejection>` |
| Custom envelopes = never-rejecting extractors + middleware rewriting 4xx bodies | return `Result<T, E>` with `E: From<Rejection> + IntoHttpResponse + ErrorSchema`; `#[derive(ApiError)]` with one `#[error(rejection)]` variant emits all three |
| No app-wide hook for 404/405/413/panic bodies | `AppBuilder::error_projection::<E>()` — the `ErrorProjector` bean (`Default` = `HttpError`), also used by routes whose return type is not a qualifying `Result<T, E>` and by SSE/WS routes |
| Default 404: empty body; default 405: no body | `404 {"error":"Not found"}`, `405 {"error":"Method not allowed"}` (`Allow` kept), `content-type: application/json`, or the `error_projection` envelope; an app `#[fallback]` still wins |
| `CatchPanicLayer::with_hook(hook)`, `catch_panic_layer_with(hook)`, `panic_response()` | `CatchPanicLayer::with(hook, projector)`, `catch_panic_layer_with(hook, projector)`; the 500 is `Rejection::internal("Internal server error")` projected (byte-equal by default) |
| OpenAPI: hardcoded 400/401/403/500 on every route, `FieldError` component; `RouteInfo.has_auth: bool` | one response per `E::status_of(kind)` for the route's inferred `rejection_kinds` + `E::extra_statuses()`, body = `E::body_schema()`; `RouteInfo { rejection_kinds, error_schema }` |
| gRPC: `GrpcGuard`, `GrpcGuardContext`, `GrpcRolesGuard`, `GrpcRoleBasedIdentity`; `#[guard]` not allowed on `#[grpc_routes]` methods | the HTTP `Guard<I>` runs on gRPC too (`#[guard]`, `#[roles]`, `#[all_roles]` on methods and impl blocks); `r2e_grpc::guard_context(..)` builds the shared `GuardContext`; `r2e_grpc::rejection_to_status(rejection)` maps by kind; `#[inject(identity)]` method params via `GrpcIdentity` (`JwtIdentitySpec` in `r2e-security`) |
| `Form<T>` body deserialization failure: `RejectionKind::InvalidForm`; `RawForm`: wrong content type `UnsupportedMediaType`, every other failure `InvalidForm` | `InvalidBody` (422, unchanged status) for a body; `InvalidForm` (400) only for a query-string form (GET/HEAD); `RawForm` content type unchanged, other failures → `PayloadTooLarge` / `BodyRead` / the kind of their status. Envelopes remapping `InvalidForm` for bodies must remap `InvalidBody` |
| `PrefixedExtract::extract_prefixed` → `Result<Self, Response>`; `#[derive(Params)]` / `TypedMultipart<T>` rejection = `Response` | `Result<Self, ParamError>`; rejection = `ParamError` / `MultipartError` (both `IntoHttpResponse` and `Into<Rejection>`) |
| MCP: `r2e_mcp::guard::guard_response_to_error(Response)` re-parsed the guard body | `McpError: From<Rejection>` by kind — `Unauthenticated`/`Forbidden` → -32600 (`data` `"unauthorized"`/`"forbidden"`), `NotFound` → -32002, request-shape kinds → -32602, `Internal`/`Unavailable`/`Timeout` → -32603, `Conflict`/`RateLimited` → tool error result |

## Step by step

1. **Guards.** Change the return type and stop building responses by hand:

   ```rust,ignore
   // 0.4
   async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Response> {
       if !ok { return Err(StatusCode::FORBIDDEN.into_response()); }
       Ok(())
   }
   // 0.5
   async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Rejection> {
       if !ok { return Err(Rejection::forbidden("not allowed")); }
       Ok(())
   }
   ```

   `Err(GuardError::new(status, msg).into())` and `Err(HttpError::forbidden(..).into())`
   also work. A guard that still has a ready-made `Response` returns
   `Err(Rejection::from(response))` (`RejectionKind::Opaque`, passed through as is
   by `HttpError`; a custom envelope decides via `opaque_passthrough()`).

2. **Managed resources.** The 0.4 bound was `type Error: Into<Response>`; set
   `type Error` to something `Into<Rejection>` instead. A type that was only
   `Into<Response>` has no automatic path — convert it to `HttpError` (or your
   envelope) in `acquire`/`finalize`; `ManagedErr<E>` only helps when `E` is
   already `Into<Rejection>`:

   ```rust,ignore
   type Error = ManagedErr<HttpError>;   // was: type Error = HttpError / Response
   ```

   Acquire and finalize failures are now projected through the route's envelope
   instead of rendering themselves.

3. **Params rejection format.** Delete `server.params-rejection-format` from your YAML
   and any `ParamsRejectionFormat` builder call. The body of a bad path/query
   param is the envelope's business; the default (`HttpError`) stays byte-equal to
   the 0.4 default format.

4. **`Via<T, M>`.** Remove the wrapper from handler parameters and custom
   extractor code: write the extractor type directly. Bean-backed extractors keep
   implementing `FromRequestPartsVia`; backend extractors keep `ViaAxum`, whose
   rejection must now be `Into<Rejection>` (all axum built-ins are).

5. **Custom error envelope** (the reason this exists — OpenAI-style bodies, RFC 9457, …).
   Add one `#[error(rejection)]` variant to your `ApiError` enum and return it from
   the routes that must use it:

   ```rust,ignore
   #[derive(Debug, ApiError)]
   pub enum ApiEnvelope {
       #[error(status = CONFLICT, message = "already exists: {0}")]
       Duplicate(String),
       #[error(rejection)]
       Rejected(Rejection),
   }

   #[post("/")]
   async fn create(&self, Json(body): Json<NewItem>) -> Result<Json<Item>, ApiEnvelope> { .. }
   ```

   A malformed body, a missing token, a denied role or a garde report on that
   route now render as `ApiEnvelope`, with the status `ApiEnvelope::status_of(kind)`
   says — and the OpenAPI spec lists exactly those statuses. To remap a status
   (say `Validation` → 422) implement `ErrorSchema` by hand instead of deriving it,
   and let `From<Rejection>` read `rejection.status` (it is already remapped).
   An enum whose only framework link is `#[error(transparent)] Http(#[from] HttpError)`
   needs no change: it inherits `HttpError`'s projection.

6. **App-wide envelope.** For routes that return something other than
   `Result<T, E>` (plain `Json<T>`, SSE, WS) and for the framework's own 404 / 405 /
   413 / panic 500, set the fallback once:

   ```rust,ignore
   AppBuilder::new()
       .error_projection::<ApiEnvelope>()   // before build_state(); default = HttpError
       .register_controller::<ItemController>()
   ```

   There is deliberately no `#[error(E)]` / `#[routes(error = E)]` attribute: a
   route declared infallible is rendered by the app-level projector.

7. **Panic layer users.** Replace `CatchPanicLayer::with_hook(hook)` with
   `CatchPanicLayer::with(hook, projector)`, where `projector` is the
   `ErrorProjector` bean (`state.get::<ErrorProjector>()`, or `ErrorProjector::default()`).

8. **Custom 404/405 bodies.** If you relied on axum's empty 404/405, nothing to do
   unless a client asserted on the empty body. If you had a `#[fallback]` route it
   still wins; the 405 fallback is only installed when R2E owns the router.

9. **gRPC.** Delete `impl GrpcGuard for X`; the same `Guard<I>` type now applies via
   `#[guard(X::new())]` on the `#[grpc_routes]` impl or method. Role checks use
   `#[roles("admin")]` with an `#[inject(identity)] user: AuthenticatedUser` parameter on the
   method (struct-level identity is a compile error on gRPC). Hand-written mappings
   call `r2e_grpc::rejection_to_status(rejection)`.

10. **MCP.** Delete any use of `guard_response_to_error`; `McpError::from(rejection)`
    carries the kind. Tests asserting on the `-32600` message text should assert on
    `data` (`"unauthorized"` / `"forbidden"`) instead.

11. **OpenAPI consumers.** Error responses now follow the route's inferred
    rejection kinds: a route with no body, identity or guard documents its
    success response plus the 500 every route can answer (`Internal`); a JSON
    body adds 400/413/415/422; a `Form<T>` body adds 400/413/415/422, a
    query-string form (GET) 400; `Path`/`Query`/`#[derive(Params)]` add 400;
    an identity — required or `Option<..>` (a present but invalid token still
    fails) — adds 401; guards add 403; a rate-limit guard adds 429. SSE and WS
    routes follow the same rules from their parameters. The panic 500 is
    documented on every route with the **application** envelope's body (the
    catch-panic layer renders with the `ErrorProjector`, not the route's
    envelope), as an `anyOf` beside the route's own 500 body when they differ.
    The body component is whatever `E::body_schema()` names — `HttpError` keeps
    `ErrorResponse` and `ValidationErrorResponse` (as an `anyOf` on 400 — a
    validation body is also a valid `ErrorResponse`); bodies are deduplicated
    by schema, and one whose name collides with a different schema is inlined
    and warned about (`SchemaGap::ErrorBodyInlined`). The `FieldError`
    component is gone.

## Where to look

- `r2e-core/src/error/rejection.rs` — `Rejection`, `RejectionKind::default_status()`,
  every `From<X> for Rejection`.
- `r2e-core/src/error/schema.rs` — `ErrorSchema`; `error/projection.rs` — `ErrorProjector`
  and the autoref probe `#[routes]` uses to pick the envelope.
- `r2e-core/tests/http/projection.rs`, `tests/http/fallback.rs`, `tests/http/panic.rs`,
  `r2e-openapi/tests/errors.rs`, `r2e-grpc/tests/guard.rs`, `r2e-mcp/tests/server/rejection.rs`
  — the behaviours above, pinned.

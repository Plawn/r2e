# Migration: error projection layers (0.4 → 0.5)

R2E is not in production, so this break (task #1072) shipped without a feature
flag. Every failure the framework raises before, around or instead of a handler —
extractor rejection, `#[inject(request)]` extraction, identity, guard, garde
validation, managed `acquire`/`finalize`, panic, 404/405/413 — is now **one typed
value, `Rejection`**, projected **once per route** into an error envelope `E`
and only then rendered. Layers connect through plain `From`/`Into`; one small
trait, `ErrorSchema`, carries the static OpenAPI metadata so the spec cannot
drift from the runtime.

Default behaviour is unchanged on the wire: `HttpError` is the default envelope
and its bodies are byte-equal to 0.4. If you never implemented a guard, a
`ManagedResource`, a gRPC guard or an MCP guard adapter, and never set
`params.rejection-format`, you rebuild and nothing moves — except the framework's
own 404/405, which now answer JSON.

Design record: `plans/error-projection.md`. User guide: `llm/error-handling.md`.

## What changed

| Old (0.4) | New (0.5) |
|---|---|
| `Guard<I>::check` / `PreAuthGuard::check` → `Result<(), Response>` (or `Result<(), GuardError>` rendered as a response) | `async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Rejection>`; `GuardError`, `RolesDenied`, `RateLimited`, `FgaDenied`, `HttpError` all convert with `?`/`.into()` |
| `ManagedResource::Error: IntoResponse` | `type Error: Into<Rejection>` — `ManagedErr<E>` wraps types you cannot write `From<E> for Rejection` for (`ManagedErr<HttpError>` is the usual choice) |
| `ParamsRejectionFormat` + `params.rejection-format` config key | removed — rejections are typed (`ParamError { location: ParamLocation, .. }` → `RejectionKind::InvalidPath` / `InvalidQuery` / `InvalidHeader` / … by location) and render through the route's envelope |
| `Via<T, M>` extractor adapter in handler signatures / custom code | removed — the generated entry fn resolves `FromRequestPartsVia` inline; the `ViaAxum` marker stays and now requires the rejection to be `Into<Rejection>` |
| Custom envelopes = never-rejecting extractors + middleware rewriting 4xx bodies | return `Result<T, E>` with `E: From<Rejection> + IntoHttpResponse + ErrorSchema`; `#[derive(ApiError)]` with one `#[error(rejection)]` variant emits all three |
| No app-wide hook for 404/405/413/panic bodies | `AppBuilder::error_projection::<E>()` — the `ErrorProjector` bean (`Default` = `HttpError`), also used by routes whose return type is not a qualifying `Result<T, E>` and by SSE/WS routes |
| Default 404: empty body; default 405: no body | `404 {"error":"Not found"}`, `405 {"error":"Method not allowed"}` (`Allow` kept), `content-type: application/json`, or the `error_projection` envelope; an app `#[fallback]` still wins |
| `CatchPanicLayer::with_hook(hook)`, `catch_panic_layer_with(hook)`, `panic_response()` | `CatchPanicLayer::with(hook, projector)`, `catch_panic_layer_with(hook, projector)`; the 500 is `Rejection::internal("Internal server error")` projected (byte-equal by default) |
| OpenAPI: hardcoded 400/401/403/500 on every route, `FieldError` component; `RouteInfo.has_auth: bool` | one response per `E::status_of(kind)` for the route's inferred `rejection_kinds` + `E::extra_statuses()`, body = `E::body_schema()`; `RouteInfo { rejection_kinds, error_schema }` |
| gRPC: `GrpcGuard`, `GrpcGuardContext`, `GrpcRolesGuard`, `GrpcRoleBasedIdentity`; `#[guard]` not allowed on `#[grpc_routes]` methods | the HTTP `Guard<I>` runs on gRPC too (`#[guard]`, `#[roles]`, `#[all_roles]` on methods and impl blocks); `r2e_grpc::guard_context(..)` builds the shared `GuardContext`; `r2e_grpc::rejection_to_status(rejection)` maps by kind; `#[inject(identity)]` method params via `GrpcIdentity` (`JwtIdentitySpec` in `r2e-security`) |
| MCP: `r2e_mcp::guard::guard_rejection_to_error(Response)` re-parsed the guard body | `McpError: From<Rejection>` by kind — `Unauthenticated`/`Forbidden` → -32600 (`data` `"unauthorized"`/`"forbidden"`), `NotFound` → -32002, request-shape kinds → -32602, `Internal`/`Unavailable`/`Timeout` → -32603, `Conflict`/`RateLimited` → tool error result |

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

2. **Managed resources.** Set `type Error` to something `Into<Rejection>`:

   ```rust,ignore
   type Error = ManagedErr<HttpError>;   // was: type Error = HttpError / Response
   ```

   Acquire and finalize failures are now projected through the route's envelope
   instead of rendering themselves.

3. **Params rejection format.** Delete `params.rejection-format` from your YAML
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

10. **MCP.** Delete any use of `guard_rejection_to_error`; `McpError::from(rejection)`
    carries the kind. Tests asserting on the `-32600` message text should assert on
    `data` (`"unauthorized"` / `"forbidden"`) instead.

11. **OpenAPI consumers.** Error responses now follow the route's inferred
    rejection kinds: a route with no body, identity or guard documents only its
    success response; a JSON body adds 400/413/415/422; identity adds 401; guards
    add 403; `#[rate_limited]` adds 429. The body component is whatever
    `E::body_schema()` names — `HttpError` keeps `ErrorResponse` and
    `ValidationErrorResponse` (as a `oneOf` on 400); the `FieldError` component is gone.

## Where to look

- `r2e-core/src/error/rejection.rs` — `Rejection`, `RejectionKind::default_status()`,
  every `From<X> for Rejection`.
- `r2e-core/src/error/schema.rs` — `ErrorSchema`; `error/projection.rs` — `ErrorProjector`
  and the autoref probe `#[routes]` uses to pick the envelope.
- `r2e-core/tests/http/projection.rs`, `tests/http/fallback.rs`, `tests/http/panic.rs`,
  `r2e-openapi/tests/errors.rs`, `r2e-grpc/tests/guard.rs`, `r2e-mcp/tests/server/rejection.rs`
  — the behaviours above, pinned.

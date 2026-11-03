# Migration: gRPC identity rides the HTTP extractor contract (r2e-grpc 0.9 → 0.10)

Released as the next minor of `r2e-grpc` and `r2e-security`, a codegen-contract
minor of `r2e-macros` (cut with `--no-propagate`; its direct consumers take a
patch re-pin) and, by propagation, the next minor of the `r2e` facade. Bump
`r2e` and run `cargo update`: that is the whole dependency change.

Task #1213 (level 2 of `plans/transport-layering.md`) removes the second
identity contract. A `#[grpc_routes]` service used to read its identity through
`r2e_grpc::GrpcIdentity`, a gRPC-only trait over the tonic `MetadataMap`. Now
every call is turned into an HTTP request head (`http::request::Parts`: method
`POST`, uri `/<package.Service>/<Method>`, the metadata as headers, the
extensions, `ConnectInfo` from the peer address) and the identity is extracted
through `FromRequestPartsVia<S, M>` exactly as on an HTTP route. Struct-level
`#[inject(identity)]` / `#[inject(request)]` fields work on gRPC services too.

If your services only use `#[inject(identity)] user: AuthenticatedUser` (or
`Option<AuthenticatedUser>`) with the `Arc<JwtClaimsValidator>` bean provided,
**they rebuild unchanged**. Read on if you implemented `GrpcIdentity`, called
`guard_context`, named `JwtIdentitySpec`, implemented `GrpcService` by hand,
or assert on the `Unauthenticated` message.

## What changed

| Old (r2e-grpc 0.9) | New (r2e-grpc 0.10) |
|---|---|
| `impl GrpcIdentity for MyIdentity { type Spec; fn spec(); fn extract(&Product, &MetadataMap) }` | `impl FromRequestParts<S>` (or a bean-backed `FromRequestPartsVia<S, ViaBean<I>>`) for `MyIdentity` — the HTTP extractor contract; the validator is a bean |
| `r2e_security::grpc::JwtIdentitySpec`, `impl GrpcIdentity for AuthenticatedUser` | removed — `AuthenticatedUser` uses its HTTP `FromRequestPartsVia` impl (`Arc<JwtClaimsValidator>` bean) |
| Identity only as a method parameter; a struct-level `#[inject(identity)]` field was a compile error | struct-level `#[inject(identity)]` and `#[inject(request)]` fields work (the method runs on the request façade); parameters keep working |
| `r2e_grpc::guard_context(&request, method, controller, identity)` | `r2e_grpc::into_head(request, fallback_uri)` then `GuardContext::from_parts(&parts, method, controller, identity, PathParams::EMPTY)` |
| `GrpcService { fn add_to_routes(routes, ctx) }` | `GrpcService<S, W = ()> { fn add_to_routes(routes, state: &S, ctx) }` |
| validator dep checked through `EndpointDeps` | checked through the witness `W` of `GrpcService<S, W>` at `register_grpc_service` |
| missing credentials (no/undecodable `authorization`, non-Bearer scheme): `Unauthenticated`, message `"Missing authorization metadata"` (or the encoding/scheme message), no `www-authenticate` | `Unauthenticated`, message `"Unauthorized"`, no `www-authenticate` |
| validator failure (bad/expired token): `Unauthenticated`, `"Unauthorized"`, `www-authenticate: Bearer` metadata | `Unauthenticated`, `"Unauthorized"`, **no** `www-authenticate` (the HTTP extractor's `HttpError` projection); a JWKS outage stays `Unavailable` |
| identity extractors could read the call's `MetadataMap` | no `MetadataMap` in the head's extensions: build `MetadataMap::from_headers(parts.headers.clone())` where needed (the headers *are* the metadata) |
| server-streaming rpcs (`type XStream`) were not forwarded by `#[grpc_routes]` | associated types are forwarded; the identity and guards run before the stream opens |

## Step by step

1. **A custom `GrpcIdentity` impl.** It no longer compiles:

   ```text
   error[E0437]: type `Spec` is not a member of trait `GrpcIdentity`
   error[E0407]: method `extract` is not a member of trait `GrpcIdentity`
   warning: use of deprecated trait `r2e::prelude::GrpcIdentity`: `GrpcIdentity` was removed:
            implement `FromRequestPartsVia` (or `FromRequestParts`) for the identity type
            instead — the validator becomes a bean. See docs/migration/grpc-identity.md
   ```

   Move the extraction to the HTTP contract. The metadata are the headers of
   the head, so `metadata.get("x-api-key")` becomes
   `parts.headers.get("x-api-key")`:

   ```rust,ignore
   // Before
   impl GrpcIdentity for ApiKeyUser {
       type Spec = ApiKeysSpec;               // DecoratorSpec, Product = Arc<ApiKeys>
       fn spec() -> Self::Spec { ApiKeysSpec }
       async fn extract(keys: &Arc<ApiKeys>, metadata: &MetadataMap) -> Result<Self, Rejection> {
           let key = metadata.get("x-api-key").and_then(|v| v.to_str().ok());
           keys.lookup(key).ok_or_else(|| HttpError::Unauthorized("bad key".into()).into())
       }
   }

   // After — the bean is read from the state, like AuthenticatedUser's validator
   impl<S, I> FromRequestPartsVia<S, ViaBean<I>> for ApiKeyUser
   where
       S: HasBean<Arc<ApiKeys>, I> + Send + Sync,
   {
       type Rejection = HttpError;

       async fn from_request_parts_via(parts: &mut Parts, state: &S) -> Result<Self, HttpError> {
           let keys: Arc<ApiKeys> = state.get_bean();
           let key = parts.headers.get("x-api-key").and_then(|v| v.to_str().ok());
           keys.lookup(key).ok_or_else(|| HttpError::Unauthorized("bad key".into()))
       }
   }
   ```

   `Option<ApiKeyUser>` parameters and fields need
   `OptionalFromRequestPartsVia` too, exactly as on HTTP. The same type now
   works on `#[routes]` handlers and `#[mcp_routes]` members unchanged. Delete
   the `impl GrpcIdentity` and the spec type.

2. **`JwtIdentitySpec`.** Delete every mention: the validator is read from the
   state. Keep providing the bean:

   ```rust,ignore
   AppBuilder::new()
       .provide(Arc::new(JwtClaimsValidator::new(..)) as Arc<JwtClaimsValidator>)
       .build_state()
       .await
       .register_grpc_service::<GreeterService>()
   ```

   Without it the registration still fails to compile, with the extractor
   error instead of the deps one:

   ```text
   error[E0277]: bean `Arc<JwtClaimsValidator>` is not present in the application state
      |             .register_grpc_service::<PingService>()
      = note: required for `AuthenticatedUser` to implement `FromRequestPartsVia<BeanState<…>, ViaBean<There<_>>>`
   note: required for `PingService` to implement `GrpcService<BeanState<…>, ((), ViaBean<There<_>>)>`
   ```

3. **Optional: move the identity to the struct.** A service whose every method
   takes the same identity can declare it once, like an HTTP controller:

   ```rust,ignore
   // Before
   #[controller]
   pub struct AccountService {}

   #[grpc_routes(proto::accounts_server::Accounts)]
   impl AccountService {
       #[roles("admin")]
       async fn close(&self, #[inject(identity)] user: AuthenticatedUser, request: Request<CloseRequest>)
           -> Result<Response<CloseReply>, Status> { .. }
   }

   // After
   #[controller]
   pub struct AccountService {
       #[inject(identity)]
       user: AuthenticatedUser,
   }

   #[grpc_routes(proto::accounts_server::Accounts)]
   impl AccountService {
       #[roles("admin")]
       async fn close(&self, request: Request<CloseRequest>) -> Result<Response<CloseReply>, Status> {
           let sub = &self.user.sub;
           ..
       }
   }
   ```

   A required struct identity authenticates every method (fail-closed). A
   service mixing public and protected methods keeps per-method parameters or
   declares `Option<AuthenticatedUser>`; `#[anonymous]` is not available on
   gRPC. Methods now run on the request façade: `Self` in a method signature
   is rejected (name the type; `Self::<Assoc>` still works), and a helper the
   methods call on `self` — sync or `async` — takes `#[request_helper]`.

4. **`guard_context`.** Code (usually a test) that built a guard context by
   hand goes through the head:

   ```rust,ignore
   // Before
   let ctx = r2e_grpc::guard_context(&request, "say_hello", "Greeter", Some(&identity));

   // After
   let (parts, _msg) = r2e_grpc::into_head(
       request,
       || "/greeter.Greeter/SayHello".parse().unwrap(),
       false,
   );
   let ctx = GuardContext::from_parts(&parts, "say_hello", "Greeter", Some(&identity), PathParams::EMPTY);
   ```

   `ctx.uri` is now the method path (it was `/`), and `ctx.peer_addr` is set
   when the call came through a transport.

5. **A hand-written `GrpcService`.** Make it generic over the state and take
   it in `add_to_routes`:

   ```rust,ignore
   // Before
   impl GrpcService for MyService {
       fn service_name() -> &'static str { "pkg.My" }
       fn add_to_routes(routes: Routes, ctx: &Arc<BeanContext>) -> Routes { .. }
   }

   // After
   impl<S: Clone + Send + Sync + 'static> GrpcService<S> for MyService {
       fn service_name() -> &'static str { "pkg.My" }
       fn add_to_routes(routes: Routes, _state: &S, ctx: &Arc<BeanContext>) -> Routes { .. }
   }
   ```

   Tests that drive `add_to_routes` directly pass `builder.state()`.

6. **Status messages.** A client or test asserting on the `Unauthenticated`
   message of *missing credentials* now sees the HTTP extractor's public
   message (`"Unauthorized"` instead of `"Missing authorization metadata"`);
   a *validator failure* keeps `"Unauthorized"` but loses its
   `www-authenticate: Bearer` metadata (the same open follow-up as HTTP: the
   `AuthenticatedUser` extractor rejects with `HttpError`). Status codes are
   unchanged.

## Unchanged

- `rejection_to_status` and its kind → code table.
- Guards, `#[roles]`, `#[all_roles]`, `#[intercept]` and the guard scoping
  (impl-level guards see `method_name = "*"`).
- The manual metadata helpers: `bearer_token`, `extract_bearer_token`,
  `extract_jwt_claims_from_metadata`, `GrpcIdentityExtractor`, and
  `JwtClaimsValidatorLike` (implemented by `JwtClaimsValidator` under
  `r2e-security`'s `grpc` feature).
- HTTP-only extractors (`Query<T>`, `Path<T>`) compile on gRPC services; the
  outcome depends on the head and the target type (`Query<T>` sees an empty
  query string, `Path<T>` tonic's `{*rest}` capture), and a rejection is a
  `Status` by kind, never a panic.

Design record: `plans/transport-layering.md` §3. User guide: `llm/grpc.md`.

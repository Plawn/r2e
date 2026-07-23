# Controller Lifetime, Identity Scope, and Handler Codegen

Reference for how `#[controller]` / `#[routes]` split a controller into an
**application-scoped core** and a **per-request façade**, where each generated
item lives, and which invariants the codegen must keep. The architecture
described here is the shipped one; the code snippets use the current API
(`ContextConstruct`, `Controller<S, W>`, `FromRequestPartsVia`).

Related docs: `di-builder-refactor.md` (HList state, `BeanLookup`,
`FromRequestPartsVia`), `beans-di.md` (`ContextConstruct`, bean graph),
`guards-interceptors.md` (`DecoratorSpec` wiring).

## The problem being solved

The source-level syntax stays convenient:

```rust
#[controller(path = "/accounts")]
pub struct AccountController {
    #[inject]
    service: AccountService,

    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl AccountController {
    #[get("/me")]
    async fn me(&self) -> Json<Account> {
        Json(self.service.load(&self.user.sub).await)
    }
}
```

`self.service` is application-scoped and must be built once. `self.user` is
request-scoped and must be freshly extracted per request. The generated Axum
handler therefore receives identity as an extractor parameter and never
reconstructs the controller:

```rust,ignore
move |request_data: __R2eRequestData_AccountController<__M>, /* Axum args */| {
    let core = captured_core.clone();
    async move {
        let request_controller =
            __r2e_meta_AccountController::bind_request(core, request_data);
        request_controller.me().await
    }
}
```

No runtime reflection, no request `Extension<Arc<Controller>>`, no task-local
identity, no duplicated handler body.

## Invariants (do not regress)

1. The physical controller core contains only application/config-scoped fields
   and is constructed once during `register_controller()`.
2. A generated request façade owns the concrete identity and an `Arc` to the
   core.
3. HTTP/SSE/WebSocket methods execute on the request façade. Their source body
   is emitted unchanged, including `self.user` and `self.service` access.
4. Core fields are reached through `Deref<Target = Core>` from the façade.
   Request-scoped fields live directly on the façade.
5. The Axum handler receives request data through `FromRequestParts`; identity
   is an explicit generated handler input.
6. The façade is a stack value. No per-request `Box`, map lookup, reflection, or
   dependency reconstruction.
7. The normal request path performs one `Arc` clone for the core.
8. A route has exactly one generated invocation body
   (`__r2e_invoke_<Controller>_<method>`, `codegen/handlers.rs`).
9. Request identity must never leak across concurrent requests.
10. Consumers and scheduled methods run on the core and cannot access request
    identity (compile error — see `compile-fail/consumer_uses_request_field.rs`).

## Rejected designs (do not reintroduce)

- CDI emulation via `tokio::task_local!`, thread-locals, a global context, or a
  `CurrentIdentity` service locator.
- `Deref<Target = AuthenticatedUser>` on a context proxy: a safe reference
  cannot be returned from an unrelated async-local context with the lifetime
  `Deref` requires.
- AST-rewriting every `self.user` expression in method bodies — fragile around
  macros, helper methods, borrows, and moves. Moving route methods to a façade
  is the supported mechanism.
- A no-state generated `Controller::routes()` compatibility path, which forced a
  second complete handler per endpoint.
- Global mutable proc-macro state to communicate between `#[controller]` and
  `#[routes]`. The liaison is stable generated type/function names only.

## Generated architecture

### 1. Physical controller core

`#[controller(...)]` is a *transforming* attribute macro (a derive could not
remove fields from its input). It consumes the source struct and emits a
physical struct without request-scoped fields:

```rust,ignore
pub struct AccountController {
    service: AccountService,
    __r2e_decos: DecoSlot,   // prebuilt #[scheduled]/#[consumer] interceptor sets
}
```

### 2. Generated request data extractor

A hidden type holds request-only values:

```rust,ignore
#[doc(hidden)]
struct __R2eRequestData_AccountController<__M> { user: AuthenticatedUser, /* … */ }
```

It is state-generic: each field is extracted through
`FromRequestPartsVia<S, M>` / `OptionalFromRequestPartsVia<S, M>` (R2E-owned,
with a marker slot carrying the `HasBean` witness — plain axum extractors reach
it through the blanket `ViaAxum` bridge). Controllers without request-scoped
fields still get a marker-only, infallible data type, so `#[routes]` generates
one uniform path without sharing proc-macro state.

### 3. Generated request façade

```rust,ignore
#[doc(hidden)]
struct __R2eRequest_AccountController {
    __core: Arc<AccountController>,
    user: AuthenticatedUser,
}

impl Deref for __R2eRequest_AccountController {
    type Target = AccountController;
    fn deref(&self) -> &Self::Target { &self.__core }
}
```

Rust field access autoderefs, so an unchanged route body resolves:

```rust,ignore
self.user       // direct request-façade field
self.service    // application-core field through Deref
```

The façade owns `Arc<Core>` (never borrows it): no lifetime parameters in Axum
futures, `Send + Sync` whenever its fields are, and it stays alive for the whole
SSE/WS future.

### 4. Binder (the inter-macro liaison)

`#[controller]` generates the binder inside `__r2e_meta_<Name>` so `#[routes]`
never needs to know façade fields:

```rust,ignore
pub fn bind_request<__M>(
    core: Arc<AccountController>,
    data: __R2eRequestData_AccountController<__M>,
) -> __R2eRequest_AccountController;
```

### 5. Method placement

`#[routes]` splits the source impl by execution scope:

- HTTP, SSE, WebSocket methods and their generated wrappers →
  `impl __R2eRequest_<Name>`;
- consumers, `#[scheduled]`, `#[post_construct]`/`#[pre_destroy]`, and ordinary
  helper methods → `impl <Name>` (the core);
- interceptor/managed-resource wrappers are emitted on the same type as the
  route method they wrap;
- `#[anonymous]` routes are emitted on the **core** (identity extraction is
  skipped entirely).

A façade route can call core helpers through `Deref` as long as those helpers do
not use request identity. Route methods that expose `Self` in their public
signature are rejected (`compile-fail/route_exposes_self.rs`) — inside a moved
method `Self` would mean the hidden façade.

### 6. Router construction

The runtime trait (`r2e-core/src/controller.rs`) has a single registration path:

```rust,ignore
pub trait Controller<T: Clone + Send + Sync + 'static, W = ()>: Send + Sync + 'static {
    type Deps;
    fn construct(state: &T, ctx: &BeanContext) -> Self;
    fn routes(state: &T, core: Arc<Self>, ctx: &BeanContext) -> Router<T>;
    // register_meta / register_consumers / scheduled_tasks_boxed / fill_decos /
    // post_construct / pre_destroy …
}
```

`W` is an opaque witness carrier where the generated impl parks inferred
extraction markers (E0207). User code never names it — `register_controller()` /
`register_controllers()` (extension traits, called after `.build_state().await`)
infer it. Route registration:

```rust,ignore
Router::new().route(
    "/me",
    get({
        let core = core.clone(); // once per registered route
        move |data: __R2eRequestData_AccountController<_>, /* args */| {
            let core = core.clone(); // once per request
            async move { bind_request(core, data).me(/* args */).await }
        }
    }),
)
```

Guards/interceptors are built once here from the `BeanContext` (see
`DecoratorSpec`) and moved into the closure. Pre-auth guards are applied while
this router is assembled — pre-auth runs **before** identity extraction. When
touching closure/Arc plumbing, verify with `cargo expand` that there is one
logical per-request `Arc` increment (Axum clones the closure, which itself
clones captured `Arc`s — an explicit clone plus a closure clone would be two).

## Lifecycle summary for docs

- application controller core: constructed once;
- generated request façade: constructed on the stack per request;
- identity: extracted once per request;
- param-level identity (`#[inject(identity)]` on a handler param): recommended
  for mixed public/protected controllers;
- struct-level identity: authenticates every endpoint (opt out per route with
  `#[anonymous]`) and does not rebuild application dependencies.

## Where the coverage lives

- Runtime behavior: `r2e-core/tests/controller/` — `core_path.rs` (core built
  once under guards/interceptors/managed/SSE/pre-auth; no controller request
  extension), `facade.rs` (concurrent identity isolation, request-scope fields,
  param/optional identity, guard sees the same identity, interceptor ordering,
  managed commit/rollback, SSE, WS), `scope.rs`, `anonymous.rs`, `config.rs`,
  `proxy_routes.rs`, `tuple.rs`.
- Compile-pass: `r2e-compile-tests/compile-pass/` —
  `identity_controller.rs`, `optional_struct_identity.rs`, `mixed_controller.rs`,
  `core_helper_from_facade.rs`, `sse_ws_identity.rs`,
  `controller_identity_guard_interceptor_managed.rs`,
  `consumer_on_identity_controller.rs`, `request_scope_field.rs`, …
- Compile-fail: `multiple_identity.rs`, `multiple_identity_params.rs`,
  `consumer_uses_request_field.rs`, `route_exposes_self.rs`,
  `old_identity_syntax.rs`, `unknown_controller_attr.rs`, `anonymous_*.rs`.
- Dispatch benchmark: `r2e-core/benches/controller_dispatch.rs`
  (`cargo bench -p r2e-core --bench controller_dispatch`) — bare axum, axum app
  stack, R2E without request scope, param identity, struct identity, all with a
  crypto-free stub identity extractor so dispatch cost is measured separately
  from JWT verification. Do not put unverified nanosecond claims in docs.

## Remaining work

- **Request-scoped helper methods** are still unsupported. A helper that needs
  `self.user` must be inlined into the route body. If this becomes a real need,
  add an explicit `#[request_helper]` marker that emits the helper on the façade
  — never infer a call graph in the proc macro. There is no compile-fail case
  for this yet; the failure is a plain "no field `user`" type error from the
  core impl block.

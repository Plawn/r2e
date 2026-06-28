# Controller Lifecycle and Handler Dispatch

This page documents how controller lifetime interacts with identity injection,
and why R2E currently generates two handler variants for each HTTP, SSE, and
WebSocket endpoint.

## Lifetime model

Controller fields do not all have the same lifetime:

| Field kind | Available from | Natural lifetime |
|------------|----------------|------------------|
| `#[inject]` | Application state | Application |
| `#[config(...)]` | Application configuration | Application |
| `#[inject(identity)]` | HTTP request credentials | Request |

A controller containing only application-scoped data can be built once when
the router is registered and safely shared as an `Arc<Controller>`.

A controller containing a struct-level identity cannot be shared between
requests. The identity belongs to one request and is stored as a concrete Rust
value in the controller. Reusing that controller concurrently would either
reuse the wrong identity or require mutable shared state. R2E therefore
extracts the identity and constructs that controller for every request.

This constraint applies to **struct-level** identity only:

```rust
#[derive(Controller)]
#[controller(state = AppState)]
struct AccountController {
    #[inject]
    service: AccountService,

    // Makes AccountController request-scoped.
    #[inject(identity)]
    user: AuthenticatedUser,
}
```

Parameter-level identity leaves the controller application-scoped:

```rust
#[derive(Controller)]
#[controller(state = AppState)]
struct AccountController {
    #[inject]
    service: AccountService,
}

#[routes]
impl AccountController {
    #[get("/me")]
    async fn me(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<Account> {
        // The controller is shared; only `user` is request-scoped.
        todo!()
    }
}
```

This is the recommended model. It makes request scope explicit in the handler,
does not reconstruct the controller, and authenticates only endpoints that
request an identity.

## Normal registration path

`AppBuilder::register_controller::<C>()` calls `C::routes_with_state(state)`.
The generated implementation selects one of two paths at router construction:

```text
register_controller
  -> routes_with_state
       -> controller without struct identity
            build C once from state
            wrap it in Arc<C>
            register closures that capture the Arc
       -> controller with struct identity
            use routes()
            extract identity and construct C on each request
```

For an application-scoped controller, one `Arc` is retained by each registered
route closure. Axum clones the closure for a request, which clones that `Arc`.
There is no request-extension lookup and no controller reconstruction on this
path.

Calling `C::routes()` directly remains supported. That path uses the generated
controller extractor. For a controller without struct identity, the extractor
first looks for an `Arc<C>` in request extensions and falls back to constructing
`C` from state if none is present. Application registration should use
`register_controller()` so the optimized path is selected.

## Why two handler variants are generated

The optimized and compatibility paths provide the controller differently:

- `routes()` needs an Axum extractor parameter;
- `routes_with_state()` needs a captured `Arc<C>` because no controller is
  stored in request extensions.

The `#[routes]` macro currently emits two functions per endpoint:

```text
__r2e_AccountController_me(
    __ctrl_ext: __R2eExtract_AccountController,
    ...
)

__r2e_AccountController_me__arc(
    __ctrl: Arc<AccountController>,
    ...
)
```

The first function backs `routes()`. The second backs the closure registered by
`routes_with_state()`. Both variants currently contain the complete generated
handler body: parameter injection, guards, interceptors, managed resources,
the controller method call, and response conversion.

This is **source generation duplication**, not two executions at runtime. A
request takes exactly one path. The costs of the current implementation are
instead paid during compilation and maintenance:

- more expanded Rust code to parse, type-check, and optimize;
- possible binary-size growth, although the linker may fold identical code;
- both variants must compile even when one is unreachable for that controller;
- route registration and pre-auth registration have parallel implementations;
- a change to generated signatures must remain synchronized with the Arc
  forwarding closures.

The duplication was introduced to preserve direct `routes()` compatibility
while removing request-extension lookup and reconstruction from the normal
application path.

## Target design: one invocation body, two thin adapters

The next iteration should keep the two controller-source adapters but emit the
handler logic only once:

```rust,ignore
async fn __invoke_me(
    controller: &AccountController,
    /* already extracted arguments */
) -> Response {
    // Guards, interceptors, managed resources, method call and conversion.
}

async fn __legacy_me(
    controller: __R2eExtract_AccountController,
    /* Axum extractors */
) -> Response {
    __invoke_me(&controller.0, /* arguments */).await
}

// Conceptual route closure; Axum owns the captured Arc for the whole await.
move |/* Axum extractors */| {
    let controller = captured_controller.clone();
    async move { __invoke_me(&controller, /* arguments */).await }
}
```

This preserves the current runtime properties:

- one controller construction at registration for application-scoped
  controllers;
- one `Arc` clone per request on the captured-controller path;
- request construction for struct-level identity controllers;
- direct `routes()` compatibility;

while reducing duplicated generated bodies. Only the small Axum-facing
adapters and their argument forwarding remain duplicated.

An alternative is to remove direct `routes()` compatibility in a future major
version and generate only the path required by the controller lifetime. That
would simplify generation further, but it is an API compatibility decision,
not a prerequisite for the shared invocation body.

## Required invariants

Changes to this dispatch mechanism must preserve these properties:

1. `register_controller()` does not insert `Arc<Controller>` into request
   extensions for application-scoped controllers.
2. Application-scoped controllers are constructed once per registration.
3. Parameter-level identity does not make the controller request-scoped.
4. Struct-level identity is extracted independently for every request.
5. Guards, interceptors, managed parameters, pre-auth guards, SSE, and
   WebSocket routes behave identically on both dispatch paths.
6. Direct `routes()` calls retain their documented fallback until that API is
   explicitly deprecated or removed.

# Controller Lifecycle and Handler Dispatch

This page documents how controller lifetime interacts with identity injection
and how the state-aware controller registration path dispatches HTTP, SSE, and
WebSocket endpoints.

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

`AppBuilder::register_controller::<C>()` calls `C::routes(&state)`. This is the
only controller route-construction API. The generated implementation selects
the controller binding at router construction:

```text
register_controller
  -> routes(&state)
       -> controller without struct identity
            build C once from state
            wrap it in Arc<C>
            register closures that capture the Arc
       -> controller with struct identity
            extract identity and construct C on each request
```

For an application-scoped controller, one `Arc` is retained by each registered
route closure. Axum clones the closure for a request, which clones that `Arc`.
There is no request-extension lookup and no controller reconstruction on this
path.

Code assembling a controller router directly must also provide state:

```rust,ignore
let router = <AccountController as Controller<AppState>>::routes(&state);
```

There is no no-argument compatibility path. Application-scoped controllers are
never looked up through request extensions and are never reconstructed as an
extractor fallback.

## One generated Axum handler

Application-scoped and request-scoped controllers are bound differently:

- the application-scoped path captures an `Arc<C>` in the registered closure;
- the struct-identity path extracts a request-scoped controller.

Both paths call the same generated Axum handler through the hidden controller
wrapper:

```text
__r2e_AccountController_me(
    __ctrl_ext: __R2eExtract_AccountController,
    ...
)
```

For an application-scoped controller, the route closure wraps its captured
`Arc<C>` directly before calling this handler. For a struct-identity controller,
Axum obtains the wrapper through `FromRequestParts`. Neither path uses a request
`Extension<Arc<C>>` lookup.

The Axum-facing handler forwards to one internal invocation function containing
parameter injection, guards, interceptors, managed resources, the controller
method call, and response conversion. HTTP, SSE, and WebSocket endpoints each
follow this shape, so there is no duplicated full handler body.

Conceptually, the expanded code is:

```rust,ignore
async fn __invoke_me(
    controller: &AccountController,
    /* already extracted arguments */
) -> Response {
    // Guards, interceptors, managed resources, method call and conversion.
}

async fn __r2e_AccountController_me(
    controller: __R2eExtract_AccountController,
    /* Axum extractors */
) -> Response {
    __invoke_me(controller.as_controller(), /* arguments */).await
}

// Application-scoped route closure; the captured Arc lives for the request/session.
move |/* Axum extractors */| {
    let controller = captured_controller.clone();
    async move {
        __r2e_AccountController_me(
            __R2eExtract_AccountController::from_application(controller),
            /* arguments */
        ).await
    }
}
```

This provides the current runtime properties:

- one controller construction at registration for application-scoped
  controllers;
- one `Arc` clone per request on the captured-controller path;
- request construction for struct-level identity controllers;
- pre-auth guards assembled within the same state-aware registration path,

with one generated Axum handler and one full invocation body per endpoint.

## Required invariants

Changes to this dispatch mechanism must preserve these properties:

1. `register_controller()` does not insert `Arc<Controller>` into request
   extensions for application-scoped controllers.
2. Application-scoped controllers are constructed once per registration.
3. Parameter-level identity does not make the controller request-scoped.
4. Struct-level identity is extracted independently for every request.
5. Guards, interceptors, managed parameters, pre-auth guards, SSE, and
   WebSocket routes behave identically on both dispatch paths.
6. Route construction always receives application state explicitly.

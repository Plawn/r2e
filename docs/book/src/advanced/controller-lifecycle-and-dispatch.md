# Controller Lifecycle and Handler Dispatch

This page documents how controller lifetime interacts with identity injection
and how the state-aware controller registration path dispatches HTTP, SSE, and
WebSocket endpoints.

## Lifetime model

Controller fields do not all have the same lifetime:

| Field kind | Available from | Natural lifetime | Lives on |
|------------|----------------|------------------|----------|
| `#[inject]` | Application state | Application | Core |
| `#[config(...)]` | Application configuration | Application | Core |
| `#[inject(identity)]` | HTTP request credentials | Request | Façade |
| `#[inject(request)]` | Any request `FromRequestParts` value | Request | Façade |

R2E splits every controller into two physical pieces:

- a **core** struct that holds only application-scoped data (`#[inject]` and
  `#[config(...)]` fields). The `#[controller]` attribute strips the
  request-scoped fields out of the emitted core, so the core can be built **once**
  when the router is registered and shared as an `Arc<Core>`.
- a generated **request façade** (`__R2eRequest_<Name>`) that holds the
  request-scoped fields and an `Arc` to the core. It implements
  `Deref<Target = Core>`, so a route body's `self.service` resolves to the core
  field through autoderef while `self.user` is a direct façade field.

Because the request façade is a stack value constructed per request, struct-level
identity **no longer reconstructs the controller's dependencies**. The core (and
everything injected into it) is built once; only the small façade — one `Arc`
clone plus the extracted identity — is created per request.

This applies to **struct-level** identity:

```rust
#[controller(state = AppState)]
struct AccountController {
    #[inject]
    service: AccountService,

    // Request-scoped: lives on the generated façade, not the core.
    #[inject(identity)]
    user: AuthenticatedUser,
}
```

Parameter-level identity is also request-scoped, but is passed as an explicit
handler argument:

```rust
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
        // The core is shared; only `user` is request-scoped.
        todo!()
    }
}
```

Parameter-level identity is the **recommended model for mixed
public/protected controllers**: it makes request scope explicit in the handler
and authenticates only the endpoints that request an identity. Struct-level
identity authenticates *every* endpoint on the controller — convenient when the
whole controller is protected — but, thanks to the façade, it no longer rebuilds
the application dependencies per request.

> Non-auth request-scoped values (a tenant id, a correlation/trace context, a
> request-scoped handle) use `#[inject(request)]` instead of
> `#[inject(identity)]`. They live on the façade exactly the same way; only
> `#[inject(identity)]` participates in guards and roles.

## Normal registration path

`AppBuilder::register_controller::<C>()` calls `C::routes(&state)`. This is the
only controller route-construction API. The generated implementation builds the
core once and captures an `Arc` of it in each registered route closure:

```text
register_controller
  -> routes(&state)
       build the core once from state (StatefulConstruct::from_state)
       wrap it in Arc<Core>
       for each route, register a closure that:
         - captures an Arc clone of the core
         - extracts __R2eRequestData_<Name> via FromRequestParts
         - binds a stack façade (bind_request)
         - invokes the route method on the façade
```

This is uniform for every controller, whether or not it declares request-scoped
fields. A controller with no request-scoped fields simply binds a façade whose
request-data extractor is zero-sized and infallible. There is no
request-extension lookup and no controller reconstruction on this path.

Code assembling a controller router directly must also provide state:

```rust,ignore
let router = <AccountController as Controller<AppState>>::routes(&state);
```

There is no no-argument compatibility path. Controllers are never looked up
through request extensions and are never reconstructed as an extractor fallback.

## One generated Axum handler

Every endpoint follows the same shape. The Axum-facing closure captures the core
`Arc`, extracts the request data, binds the façade, and forwards to one internal
invocation function containing parameter injection, guards, interceptors, managed
resources, the controller method call, and response conversion. HTTP, SSE, and
WebSocket endpoints all follow this shape, so there is no duplicated full handler
body.

Conceptually, the expanded code is:

```rust,ignore
// Built once at registration.
let core: Arc<AccountController> = Arc::new(AccountController::from_state(&state));

Router::new().route(
    "/me",
    get({
        let core = core.clone(); // once per registered route
        move |data: __R2eRequestData_AccountController, /* Axum extractors */| {
            let core = core.clone(); // one logical Arc increment per request
            async move {
                // Bind the request-scoped values into the stack façade.
                let controller = __r2e_meta_AccountController::bind_request(core, data);
                // Route body runs on the façade: self.user is a façade field,
                // self.service resolves to the core through Deref.
                controller.me(/* arguments */).await
            }
        }
    }),
)
```

`__R2eRequestData_AccountController` is the generated `FromRequestParts`
extractor that produces the request-scoped values (identity and any
`#[inject(request)]` fields). `bind_request` moves those values, together with
the core `Arc`, into the `__R2eRequest_AccountController` façade.

This provides the current runtime properties per request:

- one core construction at registration (shared across all requests);
- one `Arc` clone of the core per request;
- one `FromRequestParts` extraction binding a stack façade;
- pre-auth guards assembled within the same state-aware registration path,

with one generated Axum handler and one full invocation body per endpoint. No
request `Extension<Arc<C>>` lookup, no task-local identity, and no per-request DI
re-resolution.

## Required invariants

Changes to this dispatch mechanism must preserve these properties:

1. `register_controller()` does not insert `Arc<Controller>` into request
   extensions.
2. The controller core is constructed once per registration.
3. Parameter-level identity does not make the core request-scoped.
4. Struct-level identity (and `#[inject(request)]`) is extracted independently
   for every request into the stack façade; the core is never reconstructed.
5. Request identity never leaks across concurrent requests.
6. Guards, interceptors, managed parameters, pre-auth guards, SSE, and
   WebSocket routes behave identically through the single façade dispatch path.
7. Route construction always receives application state explicitly.

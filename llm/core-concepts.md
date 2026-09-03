---
topic: core-concepts
features: core
tokens: ~2200
requires: quick-start
---

## Core Concepts

### TL;DR

- Start every controller file with `use r2e::prelude::*;` — no direct `axum` imports; `BeanAccess` is the notable non-prelude import (`use r2e_core::type_list::BeanAccess;`).
- Declare a controller with two macros: `#[controller(path = "...")]` on the struct and `#[routes]` on the impl. There is no `state` key — controllers are state-generic.
- Four field scopes: `#[inject]` (bean, app-scoped), `#[config("key")]` (app-scoped), `#[inject(identity)]` and `#[inject(request)]` (request-scoped, both accept `Option<T>`).
- A missing `#[inject]` bean is a compile error at `register_controller()` naming the type.
- Route methods take `&self` plus extractors and return `Json<T>` or one of `ApiResult<T>` / `JsonResult<T>` / `StatusResult`.
- A plain helper method lives on the core and CANNOT read `#[inject(identity)]` / `#[inject(request)]` — `self.user` there is a "no field" compile error.
- Use `#[request_helper]` for a helper that needs request-scoped fields; it exists only on the façade, so calling it from `#[consumer]`/`#[scheduled]`/`#[anonymous]` is a compile error, and it combines with no other marker.
- Do not write `+ use<...>` on a route / `#[sse]` / `#[ws]` returning `impl Trait` — `#[routes]` inserts it; it declines when you wrote one yourself or the bounds name a lifetime/reference.

### Imports

Always start with:
```rust
use r2e::prelude::*;
```

The prelude provides everything a controller needs — no direct `axum` imports.
Highlights: macros (`controller` is used via `#[controller]`, `routes`, HTTP verbs,
`guard`, `pre_guard`, `roles`, `all_roles`, `anonymous`, `request_helper`, `intercept`,
`managed`, `consumer`, `scheduled`, `bean`, `producer`, `module`, derives `Bean`,
`DecoratorBean`, `BackgroundService`, `ProvideBundle`, `Params`, `ConfigProperties`,
`FromConfigValue`,
`Cacheable`, `ApiError`, `FromMultipart`), core types (`AppBuilder`, `HttpError`,
`R2eConfig`, `Guard`, `GuardContext`, `PreAuthGuard`, `Identity`, `Interceptor`,
`ManagedResource`, `ContextConstruct`), DI (`RegisterController`, `RegisterControllers`,
`RegisterModule`, `RegisterModules`, `BeanLookup`), plugins (`Cors`, `HttpTrace`, `Tracing`, `Health`,
`DevReload`, `NormalizePath`, `SecureHeaders`, `RequestIdPlugin`), HTTP types
(`Json`, `Router`, `StatusCode`, `HeaderMap`, `Path`, `Query`, `Form`, `State`,
`IntoHttpResponse`, `IntoResponse`, `Response`, `Redirect`, `Sse`, `SseEvent`,
`SseBroadcaster`, `SseTopic`),
type aliases (`JsonResult`, `ApiResult`, `StatusResult`), events (`EventBus`,
`LocalEventBus`), and with features: multipart (`TypedMultipart`, `UploadedFile`)
and WebSocket (`WsStream`, `WsBroadcaster`, `WsRooms`).

Not in the prelude: `BeanAccess` (`state.get::<T>()`) — import explicitly with
`use r2e_core::type_list::BeanAccess;` when needed.

### Controller Declaration

Two macros: `#[controller(...)]` on the struct, `#[routes]` on the impl block.
There is **no `state` key** — controllers are state-generic; `#[inject]` deps are
compile-checked against the app's provision list at `register_controller()`.

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,          // app-scoped, from the bean graph
    #[inject(identity)] user: AuthenticatedUser,  // request-scoped, from JWT
    #[config("app.greeting")] greeting: String,   // from R2eConfig
}
# fn main() {}
```

- `path` is optional (defaults to no prefix).
- A missing `#[inject]` bean is a **compile error at `register_controller()`** naming the type.

### Field Injection — four scopes, all compile-time

| Attribute | Scope | Requirement |
|-----------|-------|-------------|
| `#[inject]` | App-scoped | `Clone + Send + Sync + 'static`, provided/registered on the builder |
| `#[config("dotted.key")]` | App-scoped | `FromConfigValue` (`String`, ints, `f64`, `bool`, `Option<T>`, `Vec<T>`, derive for enums) |
| `#[inject(identity)]` | Request-scoped | `Identity + FromRequestParts` (e.g., `AuthenticatedUser`); drives guards/roles. `Option<T>` supported |
| `#[inject(request)]` | Request-scoped | Any `FromRequestParts` type (tenant id, trace context); `Option<T>` supported. Not in OpenAPI yet |

`#[inject(identity)]` also works on **handler parameters** for mixed
public/protected controllers (see Security).

### Route Handlers

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> { Json(self.user_service.list().await) }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, HttpError> {
        Ok(Json(self.user_service.get(id).await?))
    }

    #[post("/")]
    async fn create(&self, Json(body): Json<CreateUser>) -> JsonResult<User> {
        Ok(Json(self.user_service.create(body).await?))
    }

    #[put("/{id}")]
    async fn update(&self, Path(id): Path<u64>, Json(body): Json<UpdateUser>) -> JsonResult<User> {
        Ok(Json(self.user_service.update(id, body).await?))
    }

    #[delete("/{id}")]
    async fn delete(&self, Path(id): Path<u64>) -> StatusResult {
        self.user_service.delete(id).await?;
        Ok(StatusCode::NO_CONTENT)
    }
}
# fn main() {}
```

### Helper Methods

A plain method (no route/lifecycle marker) on a `#[routes]` impl stays on the
controller **core**. It is callable from both routes and off-request methods
(`#[consumer]`/`#[scheduled]`/`#[anonymous]`), but the core has no request-scoped
fields, so it **cannot** read `#[inject(identity)]` / `#[inject(request)]` —
`self.user` there is a compile error ("no field `user`"). Mark it or pass the
value as a parameter.

`#[request_helper]` moves the helper onto the per-request façade, next to route
methods, so it reads request-scoped fields directly and reaches `#[inject]` /
`#[config]` fields and core helpers through `Deref`. It may be `async` or take
parameters.

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}

#[routes]
impl UserController {
    // Core helper — reachable everywhere, but no identity access.
    fn label(&self, id: u64) -> String { format!("{}#{id}", self.greeting) }

    // Façade helper — reads request identity; callable ONLY from request-scoped
    // methods (routes/SSE/WS).
    #[request_helper]
    fn caller_tag(&self) -> String { format!("caller={}", self.user.sub()) }

    #[get("/{id}")]
    async fn get(&self, Path(id): Path<u64>) -> Json<String> {
        Json(format!("{} {}", self.label(id), self.caller_tag()))
    }
}
# fn main() {}
```

Intended scope boundary: a request helper does not exist on the core, so calling
it from a `#[consumer]`/`#[scheduled]`/`#[anonymous]` method is a "method not
found" compile error. `#[request_helper]` cannot be combined with a route verb /
`#[sse]` / `#[ws]` / `#[consumer]` / `#[scheduled]` / `#[async_exec]` /
`#[post_construct]` / `#[pre_destroy]` / `#[on_start]` / `#[anonymous]` /
`#[intercept]`, and is rejected on a `#[bean]` impl (no façade).

### Returning `impl Trait` — the `+ use<>` clause is automatic

A handler returning a return-position `impl Trait` needs no precise-capture
clause: `#[routes]` appends `+ use<...>` (the method's own type/const params, no
lifetimes) to every `impl Trait` in a **route / `#[sse]` / `#[ws]`** return type,
including one nested a generic argument deep (`Sse<impl Stream<..>>`). Without it
the same signature fails under edition 2024, where an `impl Trait` captures
`&self` and the value is then moved into the response — "borrowed data escapes
outside of method". So this compiles as written:

```rust
use std::convert::Infallible;
use futures_core::Stream;

#[controller(path = "/live")]
pub struct LiveController {
    #[inject] sse_broadcaster: SseBroadcaster,
}

#[routes]
impl LiveController {
    #[sse("/events")]
    async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
        self.sse_broadcaster.subscribe()
    }
}
# fn main() {}
```

The rewrite is applied to handlers only — never to `#[request_helper]`s, plain
core helpers, consumers or scheduled methods, where returning a value that
borrows `&self` is legitimate. It also **declines** (leaving the signature
untouched, so rustc prints its own suggestion) when the handler already has an
explicit `use<...>`, when the `impl Trait` bounds name a lifetime or a reference
(`impl Iterator<Item = &str>`, `+ '_`), or when the signature has an
argument-position `impl Trait`. Writing the clause by hand always wins.

### Return Type Aliases

```rust
type ApiResult<T>  = Result<T, HttpError>;
type JsonResult<T> = Result<Json<T>, HttpError>;
type StatusResult  = Result<StatusCode, HttpError>;
```

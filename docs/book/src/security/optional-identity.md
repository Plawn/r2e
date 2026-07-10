# Optional Identity

R2E supports mixed controllers where some endpoints require authentication and others do not. This is achieved through parameter-level identity injection, with optional support for endpoints that behave differently depending on whether a user is authenticated.

## Struct-level vs parameter-level identity

There are two places to inject identity in a controller:

### Struct-level identity

When `#[inject(identity)]` is on a struct field, **every endpoint** in the controller requires authentication. If the JWT is missing or invalid, the request is rejected before any handler runs.

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
    #[inject] service: UserService,
}
```

The identity field lives on the per-request façade, never on the controller core. The core still gets a `ContextConstruct` impl (it always does), so the controller can also host `#[consumer]` / `#[scheduled]` methods — those run on the core and simply cannot access the request identity.

This is the **fail-closed default** for protected controllers: every route requires auth unless explicitly opted out with `#[anonymous]`:

```rust
#[routes]
impl UserController {
    #[get("/health")]
    #[anonymous]           // public: no JWT extraction runs at all
    async fn health(&self) -> &'static str { "OK" }

    #[get("/me")]          // authenticated by default
    async fn me(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }
}
```

An `#[anonymous]` route runs on the controller core, like consumers: reading `self.user` there is a compile error, identity extraction is skipped entirely (no JWT cost), guards run with `identity: None` (or the route's own optional identity parameter, when declared), and combining the marker with `#[roles]` or a required identity parameter is rejected at compile time (an `Option<T>` identity parameter is allowed, for adaptive public routes). The marker requires a **required** struct identity — on an `Option<T>` struct identity there is nothing fail-closed to opt out of, and it is rejected. Forgetting the marker fails closed (401) — there is no marker whose omission silently publishes a route.

### Parameter-level identity

When `#[inject(identity)]` is on a handler parameter instead, only the annotated endpoints require authentication. Prefer this form for mostly-public controllers, where each endpoint opts into authentication individually.

```rust
#[controller(path = "/api")]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    // No auth required
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Item>> {
        Json(self.service.list().await)
    }

    // Auth required — injected per-handler
    #[get("/me")]
    async fn me(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<AuthenticatedUser> {
        Json(user)
    }
}
```

## Optional identity with `Option<AuthenticatedUser>`

For endpoints that should work both with and without authentication, wrap the identity type in `Option`:

```rust
#[get("/greeting")]
async fn greeting(
    &self,
    #[inject(identity)] user: Option<AuthenticatedUser>,
) -> String {
    match user {
        Some(u) => format!("Hello, {}!", u.sub()),
        None => "Hello, stranger!".to_string(),
    }
}
```

### Behavior

| Request state | Result |
|---|---|
| No `Authorization` header | `None` — handler runs normally |
| Valid JWT | `Some(AuthenticatedUser)` — user is populated |
| Invalid or expired JWT | **401 Unauthorized** — handler does not run |

This is intentional: a missing token means the caller chose not to authenticate, but a malformed token is always an error.

### How it works

R2E implements `OptionalFromRequestParts` for `AuthenticatedUser`. When the macro sees `Option<AuthenticatedUser>` annotated with `#[inject(identity)]`, it:

1. Unwraps the `Option` to determine the inner identity type.
2. Uses `OptionalFromRequestParts` instead of `FromRequestParts` for extraction.
3. Passes `None` to the handler when the `Authorization` header is absent.
4. Passes `Some(user)` when a valid JWT is present.

Guards that reference identity (via `GuardContext`) receive `Option<&AuthenticatedUser>` — they can check `.is_some()` or match on the value.

## Complete example: mixed controller

This example shows a controller with public, protected, role-gated, and optionally-authenticated endpoints:

```rust
use r2e::prelude::*;

#[controller(path = "/items")]
pub struct ItemController {
    #[inject] item_service: ItemService,
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl ItemController {
    /// Public — no identity extraction at all (`self.user` here would be a
    /// compile error).
    #[get("/")]
    #[anonymous]
    async fn list(&self) -> Json<Vec<Item>> {
        Json(self.item_service.list().await)
    }

    /// Optional auth — personalized results when logged in. The optional
    /// parameter overrides nothing: it is its own extraction, useful for
    /// adaptive endpoints on any controller shape.
    #[get("/feed")]
    #[anonymous]
    async fn feed(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> Json<Vec<Item>> {
        match user {
            Some(u) => Json(self.item_service.feed_for(&u.sub()).await),
            None => Json(self.item_service.default_feed().await),
        }
    }

    /// Protected by default — reads the struct identity, no `Option`.
    #[get("/mine")]
    async fn my_items(&self) -> Json<Vec<Item>> {
        Json(self.item_service.items_for(&self.user.sub()).await)
    }

    /// Protected + role check — the roles guard reads the struct identity;
    /// no unused parameter needed.
    #[post("/")]
    #[roles("admin")]
    async fn create(&self, body: Json<CreateItem>) -> Json<Item> {
        Json(self.item_service.create(body.0).await)
    }
}
```

## When to use which approach

| Scenario | Approach |
|---|---|
| Mostly/fully protected controller | Struct-level `#[inject(identity)]` + `#[anonymous]` on public routes (fail-closed) |
| Mostly public controller, a few protected routes | Parameter-level `#[inject(identity)]` on the protected handlers |
| Endpoint adapts to auth presence | `#[inject(identity)] user: Option<AuthenticatedUser>` parameter |
| Route requires auth but never reads the identity | Struct identity, no parameter — `#[roles]`/guards read it directly |
| Controller is also a consumer/scheduled task target | Any — the core never holds request identity |

## Combining with guards

Parameter-level identity works with `#[roles]` and `#[guard]`. The identity is extracted before guards run, so the guard context has access to it:

```rust
#[get("/admin")]
#[roles("admin")]
async fn admin_panel(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
) -> Json<AdminData> {
    // Only reachable if JWT is valid AND user has "admin" role
    Json(self.item_service.admin_data(user.sub()).await)
}
```

On a controller with a struct-level identity, the parameter is unnecessary — `#[roles]` and `#[guard]` read the struct identity directly, so a handler that never touches the user needs no identity binding at all.

With optional identity, guard context receives `Option<&AuthenticatedUser>`. Custom guards can use this to implement conditional authorization logic; on `#[anonymous]` routes the context carries `identity: None`, unless the route declares its own optional identity parameter.

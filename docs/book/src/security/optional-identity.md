# Optional Identity

R2E supports mixed controllers where some endpoints require authentication and others do not. This is achieved through parameter-level identity injection, with optional support for endpoints that behave differently depending on whether a user is authenticated.

## Struct-level vs parameter-level identity

There are two places to inject identity in a controller:

### Struct-level identity

When `#[inject(identity)]` is on a struct field, **every endpoint** in the controller requires authentication. If the JWT is missing or invalid, the request is rejected before any handler runs.

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
    #[inject] service: UserService,
}
```

This also means `StatefulConstruct` is **not** generated for the controller, so it cannot be used as a consumer or scheduled task target.

### Parameter-level identity

When `#[inject(identity)]` is on a handler parameter instead, only the annotated endpoints require authentication. The controller retains `StatefulConstruct`, making it eligible for consumers and scheduled tasks.

```rust
#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
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

#[derive(Controller)]
#[controller(path = "/items", state = AppState)]
pub struct ItemController {
    #[inject] item_service: ItemService,
}

#[routes]
impl ItemController {
    /// Public — anyone can list items.
    #[get("/")]
    async fn list(&self) -> Json<Vec<Item>> {
        Json(self.item_service.list().await)
    }

    /// Optional auth — personalized results when logged in.
    #[get("/feed")]
    async fn feed(
        &self,
        #[inject(identity)] user: Option<AuthenticatedUser>,
    ) -> Json<Vec<Item>> {
        match user {
            Some(u) => Json(self.item_service.feed_for(&u.sub()).await),
            None => Json(self.item_service.default_feed().await),
        }
    }

    /// Protected — requires a valid JWT.
    #[get("/mine")]
    async fn my_items(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<Vec<Item>> {
        Json(self.item_service.items_for(&user.sub()).await)
    }

    /// Protected + role check — admin only.
    #[post("/")]
    #[roles("admin")]
    async fn create(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
        body: Json<CreateItem>,
    ) -> Json<Item> {
        Json(self.item_service.create(body.0).await)
    }
}
```

## When to use which approach

| Scenario | Approach |
|---|---|
| Every endpoint requires auth | Struct-level `#[inject(identity)]` |
| Mix of public and protected endpoints | Parameter-level `#[inject(identity)]` |
| Endpoint adapts to auth presence | `#[inject(identity)] user: Option<AuthenticatedUser>` |
| Controller is also a consumer/scheduled task target | Parameter-level (preserves `StatefulConstruct`) |

## Combining with guards

Parameter-level identity works with `#[roles]` and `#[guard]`. The identity is extracted before guards run, so the guard context has access to it:

```rust
#[get("/admin")]
#[roles("admin")]
async fn admin_panel(
    &self,
    #[inject(identity)] _user: AuthenticatedUser,
) -> Json<AdminData> {
    // Only reachable if JWT is valid AND user has "admin" role
    Json(self.item_service.admin_data().await)
}
```

With optional identity, guard context receives `Option<&AuthenticatedUser>`. Custom guards can use this to implement conditional authorization logic.

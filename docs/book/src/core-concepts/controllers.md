# Controllers

Controllers are the central building block of R2E. They map HTTP routes to handler methods with compile-time dependency injection.

## Declaring a controller

A controller requires two macros working together:

1. `#[derive(Controller)]` on the struct — generates the Axum extractor and metadata
2. `#[routes]` on the impl block — generates Axum handler functions and route registration

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, AppError> {
        self.user_service.get_by_id(id).await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("User not found".into()))
    }

    #[post("/")]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }

    #[put("/{id}")]
    async fn update(&self, Path(id): Path<u64>, Json(body): Json<UpdateUserRequest>) -> Result<Json<User>, AppError> {
        self.user_service.update(id, body).await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("User not found".into()))
    }

    #[delete("/{id}")]
    async fn delete(&self, Path(id): Path<u64>) -> Result<(), AppError> {
        self.user_service.delete(id).await
    }
}
```

## Controller attributes

The `#[controller]` attribute takes:

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | No | URL prefix for all routes (default: `""`) |
| `state` | Yes | The application state type |

## HTTP method attributes

Mark handler methods with the HTTP method they respond to:

| Attribute | HTTP Method |
|-----------|-------------|
| `#[get("/path")]` | GET |
| `#[post("/path")]` | POST |
| `#[put("/path")]` | PUT |
| `#[delete("/path")]` | DELETE |
| `#[patch("/path")]` | PATCH |

Path parameters use `{name}` syntax and are extracted via Axum's `Path` extractor.

## Handler parameters

Handler methods receive the controller instance as `&self` plus any Axum extractors:

```rust
#[post("/{id}/comments")]
async fn add_comment(
    &self,
    Path(id): Path<u64>,                    // Path parameter
    Json(body): Json<CreateCommentRequest>,  // JSON body
    Query(params): Query<PaginationParams>,  // Query string
    headers: HeaderMap,                      // Headers
) -> Result<Json<Comment>, AppError> {
    // ...
}
```

## Injection scopes

Controller fields support three injection scopes:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject]              user_service: UserService,     // App-scoped (from state)
    #[inject(identity)]    user: AuthenticatedUser,       // Request-scoped (from request)
    #[config("app.name")]  app_name: String,              // Config-scoped (from R2eConfig)
}
```

| Scope | Attribute | Timing | Notes |
|-------|-----------|--------|-------|
| App | `#[inject]` | Per request | Cloned from state. Must be `Clone + Send + Sync`. |
| Request | `#[inject(identity)]` | Per request | Extracted from request parts. Must implement `Identity`. |
| Config | `#[config("key")]` | Per request | Looked up from `R2eConfig`. |

## Mixed controllers (param-level identity)

When only some endpoints need authentication, use param-level `#[inject(identity)]` instead of struct-level:

```rust
#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    // Public endpoint — no JWT validation
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Data>> {
        Json(self.service.public_list().await)
    }

    // Protected endpoint — JWT validated only for this handler
    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    // Optional identity — works with or without auth
    #[get("/greeting")]
    async fn greeting(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> String {
        match user {
            Some(u) => format!("Hello, {}!", u.sub),
            None => "Hello, stranger!".to_string(),
        }
    }
}
```

This is the **mixed controller pattern** — it's more efficient because JWT validation only runs on endpoints that need it. It also preserves `StatefulConstruct`, enabling the controller to be used with `#[consumer]` and `#[scheduled]`.

## Registering controllers

Controllers are registered with the application builder:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .register_controller::<UserController>()
    .register_controller::<AccountController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## What gets generated

Behind the scenes, `#[derive(Controller)]` and `#[routes]` generate:

1. **Metadata module** (`__r2e_meta_<Name>`) — state type, identity type, path prefix
2. **Extractor struct** (`__R2eExtract_<Name>`) — implements `FromRequestParts` to construct the controller
3. **StatefulConstruct impl** — when no struct-level `#[inject(identity)]` fields exist
4. **Handler functions** — standalone async functions for each route
5. **Controller trait impl** — wires routes into `axum::Router<State>`

All of this is hidden from your code — you just write the struct and methods.

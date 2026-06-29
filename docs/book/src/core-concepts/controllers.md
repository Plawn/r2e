# Controllers

Controllers are the central building block of R2E. They map HTTP routes to handler methods with compile-time dependency injection.

## Declaring a controller

A controller requires two macros working together:

1. `#[controller(path = "...", state = ...)]` on the struct — a transforming attribute that generates the controller core, the request-data extractor, the per-request façade, and metadata
2. `#[routes]` on the impl block — generates Axum handler functions and route registration

```rust
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
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, HttpError> {
        self.user_service.get_by_id(id).await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("User not found".into()))
    }

    #[post("/")]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }

    #[put("/{id}")]
    async fn update(&self, Path(id): Path<u64>, Json(body): Json<UpdateUserRequest>) -> Result<Json<User>, HttpError> {
        self.user_service.update(id, body).await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("User not found".into()))
    }

    #[delete("/{id}")]
    async fn delete(&self, Path(id): Path<u64>) -> Result<(), HttpError> {
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
    Json(body): Json<CreateCommentRequest>,  // JSON body (auto-validated if T: Validate)
    Query(params): Query<PaginationParams>,  // Query string
    headers: HeaderMap,                      // Headers
) -> Result<Json<Comment>, HttpError> {
    // ...
}
```

You can also aggregate multiple parameter sources into a single struct using `#[derive(Params)]`:

```rust
#[derive(Params)]
pub struct CommentParams {
    #[path]
    pub id: u64,
    #[query]
    pub page: Option<u32>,
    #[header("X-Tenant-Id")]
    pub tenant_id: String,
}

#[get("/{id}/comments")]
async fn list_comments(&self, params: CommentParams) -> Json<Vec<Comment>> {
    // params.id, params.page, params.tenant_id extracted automatically
}
```

See [Validation](./validation.md#params--aggregated-parameter-extraction) for details on `#[derive(Params)]` and its integration with garde validation.

## Injection scopes

Controller fields support four injection scopes — two app-scoped (on the core)
and two request-scoped (on the per-request façade):

```rust
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject]              user_service: UserService,     // App-scoped (from state)
    #[config("app.name")]  app_name: String,              // App-scoped (from R2eConfig)
    #[inject(identity)]    user: AuthenticatedUser,        // Request-scoped auth identity
    #[inject(request)]     tenant: TenantId,               // Request-scoped (any FromRequestParts)
}
```

| Scope | Attribute | Lives on | Timing | Notes |
|-------|-----------|----------|--------|-------|
| App | `#[inject]` | Core | Built once | Cloned from state. Must be `Clone + Send + Sync`. |
| Config | `#[config("key")]` | Core | Built once | Looked up from `R2eConfig`. |
| Request (identity) | `#[inject(identity)]` | Façade | Per request | Extracted from request parts. Must implement `Identity`. Drives guards/roles. |
| Request (generic) | `#[inject(request)]` | Façade | Per request | Any `FromRequestParts` value (tenant id, trace context, request-scoped handle). Not modeled in OpenAPI yet. |

`Option<T>` is supported for both `#[inject(identity)]` and `#[inject(request)]`.

The app-scoped fields live on a physical **core** struct built once when the
router is registered; the request-scoped fields live on a generated per-request
**façade** that `Deref`s to the core. As a result, struct-level identity does
**not** rebuild the controller's dependencies per request — only the small façade
(one `Arc` clone of the core plus the extracted request values) is created per
request. See [Controller Lifecycle and Handler Dispatch](../advanced/controller-lifecycle-and-dispatch.md).

**Important:** Struct-level `#[inject(identity)]` means **all** endpoints require authentication. For mixed public/protected controllers, use param-level injection instead (see below).

## Mixed controllers (param-level identity)

When only some endpoints need authentication, use param-level `#[inject(identity)]` instead of struct-level. This is the **recommended pattern** for most controllers:

```rust
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

This is the **mixed controller pattern** — it's more efficient because JWT validation only runs on endpoints that need it, and it keeps request scope explicit per handler. The controller core always implements `StatefulConstruct`, so it can also be used with `#[consumer]` and `#[scheduled]` (which run on the core and cannot access request identity).

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

Behind the scenes, `#[controller]` and `#[routes]` generate:

1. **Controller core** — your struct with request-scoped fields stripped out; holds only `#[inject]` + `#[config]` fields and is built once into an `Arc`
2. **Metadata module** (`__r2e_meta_<Name>`) — state type, identity type, path prefix, `bind_request`, config validation
3. **Request-data extractor** (`__R2eRequestData_<Name>`) — implements `FromRequestParts` to extract the request-scoped values (identity + `#[inject(request)]`)
4. **Request façade** (`__R2eRequest_<Name>`) — `{ __core: Arc<Core>, <request fields> }` with `Deref<Target = Core>`; route methods run here
5. **StatefulConstruct impl** — always generated (the core never holds request-scoped fields)
6. **Controller trait impl** — wires the core supplied by `register_controller()` into routes, consumers, and scheduled tasks

All of this is hidden from your code — you just write the struct and methods.

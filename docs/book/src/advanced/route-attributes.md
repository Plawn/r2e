# Route Attributes

R2E provides several route-level attributes that go beyond the standard `#[get]`, `#[post]`, etc. These give you fine-grained control over middleware, OpenAPI metadata, and Axum extractor usage on individual routes or entire controllers.

All attributes described here are re-exported from the prelude:

```rust
use r2e::prelude::*; // middleware, layer, status, returns, raw
```

---

## `#[middleware]` — Tower middleware functions

The `#[middleware]` attribute applies a Tower middleware function to a specific route. It wraps the handler with `MethodRoutes::middleware_fn`, so the function must follow the standard middleware signature.

### Middleware function signature

A middleware function takes a `Request` and `Next`, and returns a `Response`:

```rust
use r2e::prelude::*;

async fn require_api_key(
    req: Request,
    next: r2e::http::middleware::Next,
) -> Response {
    let has_key = req
        .headers()
        .get("x-api-key")
        .map_or(false, |v| v == "secret");

    if !has_key {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    next.run(req).await
}
```

### Applying to a single route

```rust
#[routes]
impl ApiController {
    #[get("/data")]
    #[middleware(require_api_key)]
    async fn protected_data(&self) -> Json<Vec<Item>> {
        Json(self.service.list().await)
    }
}
```

The generated code calls `.middleware_fn(require_api_key)` on that route's `MethodRoutes`.

### Multiple middleware

You can stack multiple `#[middleware]` attributes. Each one becomes a `.layer()` call in declaration order, and the last layer applied is the outermost — so the **last-declared** middleware runs first:

```rust
#[get("/data")]
#[middleware(require_api_key)]
#[middleware(log_request)]
async fn protected_data(&self) -> Json<Vec<Item>> {
    // log_request runs first, then require_api_key, then the handler
    Json(self.service.list().await)
}
```

### Middleware with the controller core: `state`

`#[middleware(my_fn, state)]` passes the controller core to the function as its first argument, `Arc<Self>` (via `MethodRoutes::middleware_fn_with_state`). The controller's `#[inject]` beans and `#[config]` values are available; request-scoped fields (identity, `#[inject(request)]`) are not:

```rust
use std::sync::Arc;

async fn tag_response(
    ctrl: Arc<ApiController>,
    req: r2e::http::Request,
    next: r2e::http::middleware::Next,
) -> r2e::http::response::Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert("x-api-tag", ctrl.tag.parse().unwrap());
    res
}

#[controller(path = "/api")]
pub struct ApiController {
    #[config("app.tag")]
    tag: String,
}

#[routes]
impl ApiController {
    #[get("/ping")]
    #[middleware(tag_response, state)]
    async fn ping(&self) -> &'static str { "pong" }
}
```

---

## `#[layer]` — Arbitrary Tower layers

While `#[middleware]` wraps a plain function, `#[layer]` accepts any expression that evaluates to a Tower `Layer`. This lets you use the `tower-http` layers R2E re-exports under `r2e::http::layers` and any other Tower-compatible crate directly.

### Basic usage

```rust
use r2e::http::layers::timeout::TimeoutLayer;
use std::time::Duration;

#[routes]
impl ApiController {
    #[get("/slow")]
    #[layer(TimeoutLayer::new(Duration::from_secs(5)))]
    async fn slow_operation(&self) -> Json<&'static str> {
        // If this takes longer than 5 seconds, the layer returns a timeout error
        Json("done")
    }
}
```

### Using `tower-http` layers with status codes

The `TimeoutLayer` from `tower-http` supports a custom status code variant:

```rust
#[get("/slow")]
#[layer(r2e::http::layers::timeout::TimeoutLayer::with_status_code(
    r2e::http::StatusCode::REQUEST_TIMEOUT,
    std::time::Duration::from_secs(2),
))]
async fn slow_endpoint(&self) -> Json<&'static str> {
    Json("done")
}
```

### Combining with `#[middleware]`

Both attributes can be used on the same route. They are all applied as `.layer()` calls on the route handler:

```rust
#[get("/data")]
#[middleware(require_api_key)]
#[layer(TimeoutLayer::new(Duration::from_secs(10)))]
async fn guarded_with_timeout(&self) -> Json<Data> {
    Json(self.service.fetch().await)
}
```

### Building a layer from the controller: `|this: &Self|`

A closure taking the controller core builds the layer from its beans or config. It is called **once**, when the route is registered — never per request:

```rust
#[controller(path = "/upload")]
pub struct UploadController {
    #[config("upload.max-bytes")]
    max_bytes: usize,
}

#[routes]
impl UploadController {
    #[post("/")]
    #[layer(|this: &Self| DefaultBodyLimit::max(this.max_bytes))]
    async fn upload(&self, body: Bytes) -> StatusCode { StatusCode::CREATED }
}
```

### Common layers

Here are some frequently used Tower layers:

| Layer | Crate | Purpose |
|-------|-------|---------|
| `TimeoutLayer` | `r2e::http::layers::timeout` | Request timeout |
| `SetResponseHeaderLayer` | `r2e::http::layers::set_header` | Add response headers |
| `CorsLayer` | `r2e::http::layers::cors` | CORS configuration |
| `CompressionLayer` | `tower-http` (not re-exported — depend on it yourself) | Response compression |
| `ConcurrencyLimitLayer` | `tower` | Limit concurrent requests |

---

## Controller-level `#[middleware]` / `#[layer]`

Place `#[middleware]` or `#[layer]` on the impl block, **below** `#[routes]`, to wrap every route of the controller — regular routes, `#[sse]` and `#[ws]` methods alike. All the forms above work (`state`, the `|this: &Self|` closure); an impl-level layer is built once per controller and cloned onto each route.

```rust
#[controller(path = "/proxy")]
pub struct ProxyController {
    #[config("proxy.request-body-limit-bytes")]
    body_limit: usize,
}

#[routes]
#[middleware(audit, state)]
#[layer(|this: &Self| DefaultBodyLimit::max(this.body_limit))]
impl ProxyController {
    #[post("/forward")]
    async fn forward(&self, body: String) -> String { body }

    #[put("/forward")]
    async fn replace(&self, body: String) -> String { body }
}
```

Rules:

- **Order** — per route, the method's `#[middleware]`s and `#[layer]`s are applied first, then the controller's: controller layers are the **outer** ones. Within a level, a later-declared attribute wraps an earlier one, and `#[layer]`s wrap `#[middleware]`s.
- **`#[fallback]` is skipped** — `Routes::fallback` takes no per-route layers. If the impl block has no other route, the impl-level attribute is a compile error (it would apply to nothing).
- **Body limits keep the envelope** — the limit is enforced by the body extractor inside the handler, so an oversized body answers `413` through the route's error type, like any other extraction rejection.
- `#[grpc_routes]` and `#[mcp_routes]` do not accept `#[middleware]`/`#[layer]`.

---

## `#[status]` — Override HTTP status code

By default, R2E assigns a conventional HTTP status code to each route method for OpenAPI documentation:

| HTTP method | Default status |
|-------------|---------------|
| `GET`       | 200           |
| `POST`      | 201           |
| `PUT`       | 200           |
| `PATCH`     | 200           |
| `DELETE`    | 204           |

Use `#[status(N)]` to override the default when your endpoint deviates from these conventions.

### When to use it

The status code override affects the **OpenAPI specification** generated by `r2e-openapi`. It tells the documentation what the expected success status code is. It does not change the actual HTTP response your handler returns -- that is still determined by your return type and `IntoResponse` implementation.

### Examples

Override a `DELETE` that returns a body (default is 204 No Content):

```rust
#[delete("/users/{id}")]
#[status(200)]
async fn delete_user(
    &self,
    Path(id): Path<u64>,
) -> JsonResult<DeleteConfirmation> {
    let confirmation = self.service.delete(id).await?;
    Ok(Json(confirmation))
}
```

Override a `POST` that does not create a resource:

```rust
#[post("/users/search")]
#[status(200)]
async fn search_users(
    &self,
    body: Json<SearchQuery>,
) -> JsonResult<Vec<User>> {
    let results = self.service.search(&body).await?;
    Ok(Json(results))
}
```

Mark a `POST` endpoint that returns 202 Accepted for async processing:

```rust
#[post("/jobs")]
#[status(202)]
async fn submit_job(
    &self,
    body: Json<JobRequest>,
) -> JsonResult<JobId> {
    let id = self.service.enqueue(body.0).await?;
    Ok(Json(id))
}
```

---

## `#[returns]` — Explicit response type for OpenAPI

R2E's OpenAPI generator probes the handler's return type for the `ResponseBodySchema` trait, which the framework implements for its own types (`Json<T>`, `String`, `Html<T>`, `Bytes`, `()`, `StatusCode`, `Sse<S>`, …) and which delegates through `Result<T, E>` — so `JsonResult<T>`, `ApiResult<Json<T>>` and your own aliases work. However, when you return an opaque type such as `impl IntoResponse`, there is no type to probe.

Use `#[returns(T)]` to explicitly tell the OpenAPI generator what the response body is. `T` is probed for `ResponseBodySchema` first, then as `Json<T>`: a `JsonSchema` DTO documents an `application/json` body, a custom `ResponseBodySchema` type documents every media type it lists.

### When to use it

- Your handler returns `impl IntoResponse`
- You want the OpenAPI schema to reflect a different type than the literal return type

A concrete custom response type does not need `#[returns]`: implement `ResponseBodySchema` on it instead and it is documented wherever it is returned.

### Examples

Annotating a handler that returns `impl IntoResponse`:

```rust
#[get("/widgets/{id}")]
#[returns(Widget)]
async fn get_widget(
    &self,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let widget = self.service.find(id).await;
    Json(widget)
}
```

The OpenAPI spec will show `Widget` as the response schema for this endpoint, just as if the handler returned `Json<Widget>` directly.

Annotating a handler with a custom response builder:

```rust
#[get("/report")]
#[returns(Report)]
#[status(200)]
async fn generate_report(&self) -> impl IntoResponse {
    let report = self.service.build_report().await;
    (StatusCode::OK, Json(report))
}
```

### Combining with `#[status]`

`#[returns]` and `#[status]` work well together for complete OpenAPI control:

```rust
#[post("/orders")]
#[status(202)]
#[returns(OrderReceipt)]
async fn place_order(
    &self,
    body: Json<OrderRequest>,
) -> impl IntoResponse {
    let receipt = self.service.place(body.0).await;
    (StatusCode::ACCEPTED, Json(receipt))
}
```

This produces an OpenAPI operation with a `202` response containing an `OrderReceipt` schema.

---

## `#[raw]` — Marking raw Axum extractors

All handler parameters that are not annotated with `#[inject(identity)]` or `#[managed]` are already treated as raw Axum extractors and passed directly to the generated handler function. The `#[raw]` attribute is a **documentation-only marker** -- it has no effect on code generation. Its purpose is to make intent explicit when using uncommon extractors.

### When to use it

Use `#[raw]` when a handler parameter is an Axum extractor that might be confused with an R2E-managed type, or when you want to signal to other developers that the parameter is intentionally passed through as-is.

### Examples

Accessing client connection info:

```rust
use std::net::SocketAddr;
use r2e::http::ConnectInfo;

#[get("/")]
async fn handler(
    &self,
    #[raw] connect_info: ConnectInfo<SocketAddr>,
    #[raw] headers: HeaderMap,
) -> Json<&'static str> {
    let addr = connect_info.0;
    tracing::info!("Request from {}", addr);
    Json("hello")
}
```

Using a custom Axum extractor alongside injected dependencies:

```rust
#[post("/upload")]
async fn upload(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
    #[raw] multipart: Multipart,
) -> Result<Json<UploadResult>, HttpError> {
    // user is extracted via R2E identity injection
    // multipart is extracted directly by Axum
    self.service.handle_upload(&user, multipart).await
}
```

### Without `#[raw]`

The following is functionally identical -- `#[raw]` changes nothing at runtime:

```rust
#[get("/")]
async fn handler(
    &self,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Query(params): Query<ListParams>,
) -> Json<Vec<Item>> {
    // headers, Path, Query are all raw Axum extractors regardless
    Json(self.service.list(id, params).await)
}
```

The `#[raw]` marker simply makes it clear that a parameter is an escape hatch from R2E's injection system, which is useful for code review and documentation.

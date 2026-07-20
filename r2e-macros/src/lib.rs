extern crate proc_macro;
use proc_macro::TokenStream;

pub(crate) mod api_error_derive;
pub(crate) mod bean_attr;
pub(crate) mod bean_derive;
pub(crate) mod bg_service_derive;
pub(crate) mod cacheable_derive;
pub(crate) mod codegen;
pub(crate) mod config_derive;
pub(crate) mod controller_attr;
pub(crate) mod controller_codegen;
pub(crate) mod controller_parsing;
pub(crate) mod crate_path;
pub(crate) mod decorator_bean_derive;
pub(crate) mod extract;
pub(crate) mod field_resolver;
pub(crate) mod from_config_value_derive;
pub(crate) mod from_multipart;
pub(crate) mod grpc_codegen;
pub(crate) mod grpc_routes_parsing;
pub(crate) mod hash_tokens;
pub(crate) mod main_attr;
pub(crate) mod module_attr;
pub(crate) mod params_derive;
pub(crate) mod producer_attr;
pub(crate) mod route;
pub(crate) mod routes_attr;
pub(crate) mod routes_parsing;
pub(crate) mod type_list_gen;
pub(crate) mod type_utils;
pub(crate) mod types;

/// Attribute macro for declaring a R2E controller struct.
///
/// # Struct-level attribute
///
/// `#[controller(...)]` configures the controller:
///
/// | Parameter | Required | Description |
/// |-----------|----------|-------------|
/// | `state`   | **yes**  | The Axum application state type (e.g. `Services`) |
/// | `path`    | no       | Route prefix applied to every route in this controller |
///
/// # Field attributes
///
/// | Attribute | Scope | Description |
/// |-----------|-------|-------------|
/// | `#[inject]` | App-scoped | Cloned from the Axum state. Type must impl `Clone + Send + Sync`. |
/// | `#[inject(identity)]` | Request-scoped | Extracted via Axum's `FromRequestParts` (e.g. `AuthenticatedUser`). Type must impl `Identity`. |
/// | `#[config("key")]` | App-scoped | Resolved from `R2eConfig` at registration. |
///
/// # Handler parameter attributes
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[inject(identity)]` | Marks a handler parameter as the identity source for guards. Enables mixed controllers (public + protected endpoints). |
///
/// # Example
///
/// ```ignore
/// use r2e_core::prelude::*;
///
/// // Struct-level identity (all endpoints require auth)
/// #[controller(path = "/users", state = Services)]
/// pub struct UserController {
///     #[inject]  user_service: UserService,
///     #[inject]  pool: sqlx::SqlitePool,
///     #[inject(identity)] user: AuthenticatedUser,
///     #[config("app.greeting")] greeting: String,
/// }
///
/// // Mixed controller (param-level identity)
/// #[controller(path = "/api", state = Services)]
/// pub struct MixedController {
///     #[inject] user_service: UserService,
///     // No identity on struct → ContextConstruct is generated
/// }
///
/// #[routes]
/// impl MixedController {
///     #[get("/public")]
///     async fn public_data(&self) -> Json<Vec<Data>> { ... }
///
///     #[get("/me")]
///     async fn me(
///         &self,
///         #[inject(identity)] user: AuthenticatedUser,
///     ) -> Json<AuthenticatedUser> {
///         Json(user)
///     }
/// }
/// ```
///
/// # What is generated
///
/// The struct is rewritten into a **physical core** holding only app/config
/// fields — every request-scoped field (`#[inject(identity)]` and
/// `#[inject(request)]`) is removed and lives on a generated façade instead.
///
/// - A hidden metadata module (`__r2e_meta_<Name>`) — state type alias, path
///   prefix, identity type alias, the `guard_identity` accessor (reads the
///   façade) and `bind_request` (binds the façade).
/// - A request-data extractor (`__R2eRequestData_<Name>`) implementing
///   `FromRequestParts<State>` — extracts the request-scoped values (zero-sized
///   and infallible when there are none).
/// - The request façade (`__R2eRequest_<Name>`) — owns `Arc<Name>` plus the
///   request-scoped values, with `Deref<Target = Name>`. Route methods run on
///   it; app/config fields and core helpers are reached through `Deref`.
/// - `impl ContextConstruct<State> for Name` — **always** (the core has no
///   request-scoped fields), so the core is built once per registration and
///   reused by every request, consumer, and scheduled task.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller_attr::expand(args, input)
}

/// Attribute macro on an `impl` block — generates Axum handlers, route
/// wiring, and trait impls.
///
/// Must be placed on an `impl` block whose `Self` type uses [`macro@controller`].
///
/// # Method attributes
///
/// Inside the impl block you can annotate methods with:
///
/// - **HTTP routes**: [`get`], [`post`], [`put`], [`delete`], [`patch`]
/// - **Authorization**: [`roles`]
/// - **Interceptors**: [`intercept`]
/// - **Events**: [`consumer`]
/// - **Scheduling**: [`scheduled`]
/// - **Guards**: [`guard`], [`pre_guard`]
/// - **Rate limiting**: Use `#[guard(RateLimit::per_user(...))]` or `#[pre_guard(RateLimit::global(...))]`
/// - **Middleware**: [`middleware`]
///
/// # Block-level interceptors
///
/// Place `#[intercept(...)]` on the `impl` block itself to apply an
/// interceptor to **every** route method:
///
/// ```ignore
/// #[routes]
/// #[intercept(Logged::info())]
/// impl UserController {
///     #[get("/")]
///     async fn list(&self) -> Json<Vec<User>> { ... }
///
///     #[post("/")]
///     async fn create(&self, Json(body): Json<CreateUser>) -> Json<User> { ... }
/// }
/// ```
///
/// # What is generated
///
/// - The original `impl` block with method bodies wrapped by interceptors.
/// - Free-standing Axum handler functions (`__r2e_<Name>_<method>`).
/// - `impl Controller<State> for Name` — route registration, metadata,
///   consumer registration, and scheduled task definitions.
#[proc_macro_attribute]
pub fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
    routes_attr::expand(input)
}

// ---------------------------------------------------------------------------
// No-op attributes — consumed by #[routes] from the token stream.
// Declared here for IDE support (rust-analyzer), cargo doc, and to prevent
// "cannot find attribute" errors when used outside #[routes].
// ---------------------------------------------------------------------------

/// Register a **GET** route handler.
///
/// The path argument supports static segments and parameters (Axum syntax):
///
/// ```ignore
/// #[get("/users")]             // static path
/// #[get("/users/{id}")]        // path parameter
/// #[get("/users/{id}/posts")]  // nested
/// ```
///
/// When the controller has `#[controller(path = "/api")]`, the paths are
/// nested: `/api/users`, `/api/users/{id}`, etc.
///
/// # Return type aliases
///
/// ```ignore
/// use r2e_core::prelude::*;
///
/// // With type alias (recommended)
/// #[get("/users")]
/// async fn list(&self) -> JsonResult<Vec<User>> {
///     Ok(Json(self.service.list().await))
/// }
///
/// // Direct type (equivalent)
/// #[get("/users/{id}")]
/// async fn get(&self, Path(id): Path<u64>) -> Result<Json<User>, HttpError> { ... }
///
/// // Any response type
/// #[get("/health")]
/// async fn health(&self) -> ApiResult<StatusCode> {
///     Ok(StatusCode::OK)
/// }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn get(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a **POST** route handler.
///
/// ```ignore
/// #[post("/users")]
/// async fn create(&self, Json(body): Json<CreateUser>) -> JsonResult<User> {
///     // body is automatically validated if CreateUser derives garde::Validate
///     Ok(Json(self.service.create(body).await?))
/// }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn post(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a **PUT** route handler.
///
/// ```ignore
/// #[put("/users/{id}")]
/// async fn update(&self, Path(id): Path<u64>, body: Json<UpdateUser>) -> JsonResult<User> {
///     Ok(Json(self.service.update(id, body.0).await?))
/// }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn put(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a **DELETE** route handler.
///
/// ```ignore
/// // With type alias
/// #[delete("/users/{id}")]
/// async fn delete(&self, Path(id): Path<u64>) -> StatusResult {
///     self.service.delete(id).await?;
///     Ok(StatusCode::NO_CONTENT)
/// }
///
/// // Direct type (equivalent)
/// #[delete("/users/{id}")]
/// async fn delete(&self, Path(id): Path<u64>) -> StatusCode { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn delete(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a **PATCH** route handler.
///
/// ```ignore
/// #[patch("/users/{id}")]
/// async fn patch(&self, Path(id): Path<u64>, body: Json<PatchUser>) -> JsonResult<User> {
///     Ok(Json(self.service.patch(id, body.0).await?))
/// }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn patch(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a route handler matching **every HTTP method** (`axum::routing::any`).
///
/// Combine with a `{*wildcard}` path segment for proxy-shaped endpoints, and
/// take the raw [`Request`] as the **last** parameter to access the method,
/// headers, and streaming body:
///
/// ```ignore
/// #[any("/proxy/{*path}")]
/// async fn proxy(&self, req: Request) -> Response {
///     // route on req.method() / req.uri(), stream the body
/// }
/// ```
///
/// `#[any]` routes are excluded from the OpenAPI spec (they have no single
/// method or response shape to document). Guards, interceptors, and DI work
/// exactly as on verb routes.
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn any(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Register a **controller-scoped catch-all**: handles every request no other
/// route matched, for any HTTP method (`axum::Router::fallback`).
///
/// ```ignore
/// #[fallback]
/// async fn dispatch(&self, req: Request) -> Response {
///     // registry-protocol dispatch, proxying, custom 404s...
/// }
/// ```
///
/// Rules:
/// - takes **no path argument** — it matches whatever is left over;
/// - at most **one** `#[fallback]` per controller;
/// - only allowed on controllers **without a path prefix** (the fallback is
///   app-wide; a prefixed fallback would be misleading);
/// - if two registered controllers both declare one, the router build panics
///   (axum allows a single fallback per app);
/// - excluded from the OpenAPI spec.
///
/// Declared routes always win: the fallback only sees unmatched requests.
/// Guards, interceptors, and DI work exactly as on verb routes.
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn fallback(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Restrict a route to users that have **at least one** of the specified roles.
///
/// Requires an identity source: either an `#[inject(identity)]` field on the
/// controller struct, or an `#[inject(identity)]` parameter on the handler.
/// Returns **403 Forbidden** if the user lacks every listed role.
///
/// ```ignore
/// #[get("/admin/users")]
/// #[roles("admin")]
/// async fn admin_list(&self) -> Json<Vec<User>> { ... }
///
/// #[get("/manager/reports")]
/// #[roles("admin", "manager")]   // OR — any one role suffices
/// async fn reports(&self) -> Json<Vec<Report>> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn roles(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Opt a route out of the controller's struct-level identity (@PermitAll).
///
/// On a controller with an `#[inject(identity)]` field, **every** route
/// requires authentication by default (fail-closed). Mark the public
/// exceptions with `#[anonymous]`:
///
/// ```ignore
/// #[controller(path = "/posts")]
/// pub struct PostController {
///     #[inject] posts: PostService,
///     #[inject(identity)] user: AuthenticatedUser,
/// }
///
/// #[routes]
/// impl PostController {
///     #[get("/")]
///     #[anonymous]               // public: no JWT extraction at all
///     async fn list(&self) -> Json<Vec<Post>> { ... }
///
///     #[post("/")]               // authenticated by default
///     async fn create(&self, body: Json<NewPost>) -> Json<Post> {
///         let owner = self.user.sub();
///         ...
///     }
/// }
/// ```
///
/// Anonymous routes run on the controller **core** (like `#[consumer]` /
/// `#[scheduled]` methods): identity extraction is skipped entirely — no JWT
/// validation cost — and reading the identity field (or any request-scoped
/// field) in the route body is a compile error. Handler parameters and
/// `#[inject]`/`#[config]` fields work as usual.
///
/// Not combinable with `#[roles]`/`#[all_roles]` or a **required**
/// `#[inject(identity)]` parameter (those require an identity). An `Option<T>`
/// identity parameter is allowed — it makes a public route adaptive
/// (personalized when a valid credential is present). The controller must
/// declare a **required** struct-level identity: with no identity the routes
/// are already public, and an `Option<T>` identity never rejects — in both
/// cases the marker is rejected at compile time.
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn anonymous(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Restrict a route to users that have **all** of the specified roles (AND semantics).
///
/// Requires an identity source: either an `#[inject(identity)]` field on the
/// controller struct, or an `#[inject(identity)]` parameter on the handler.
/// Returns **403 Forbidden** if the user is missing any of the listed roles.
///
/// ```ignore
/// #[get("/admin/settings")]
/// #[all_roles("admin", "superadmin")]   // AND — user must have BOTH roles
/// async fn admin_settings(&self) -> Json<Settings> { ... }
/// ```
///
/// For OR semantics (require **at least one** role), use [`roles`] instead.
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn all_roles(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply an interceptor (cross-cutting concern) to a method or an entire
/// `impl` block.
///
/// Interceptors implement [`r2e_core::Interceptor<R>`] and wrap the
/// method body with an `around` pattern (logging, timing, caching, etc.).
///
/// **Built-in interceptors** (from `r2e_utils::interceptors`):
///
/// ```ignore
/// #[intercept(Logged::info())]            // log entering / exiting
/// #[intercept(Logged::debug())]           // custom level
/// #[intercept(Timed::info())]             // measure execution time
/// #[intercept(Timed::threshold(100))]     // only log if > 100 ms
/// #[intercept(Cache::ttl(30))]            // cache JSON response 30 s
/// #[intercept(Cache::ttl(60).group("users"))]  // named cache group
/// #[intercept(CacheInvalidate::group("users"))] // clear cache after exec
/// ```
///
/// **User-defined interceptors** — any unit struct / constant implementing
/// `Interceptor<R>`:
///
/// ```ignore
/// #[intercept(AuditLog)]
/// ```
///
/// When placed on the `impl` block, the interceptor applies to **every**
/// route method in that block.
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn intercept(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a custom guard to a route method.
///
/// Guards run **before** the handler body and can short-circuit with an
/// error response. The type must implement [`r2e_core::Guard`].
///
/// ```ignore
/// #[guard(MyCustomGuard)]
/// #[get("/protected")]
/// async fn protected(&self) -> Json<Data> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn guard(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a custom pre-authentication guard to a route method.
///
/// Pre-auth guards run as middleware **before** JWT extraction/validation.
/// The type must implement [`r2e_core::PreAuthGuard`].
///
/// Use this for authorization checks that don't need identity (e.g.,
/// IP-based allowlisting, custom rate limiting).
///
/// ```ignore
/// #[pre_guard(MyIpAllowlistGuard)]
/// #[get("/api/data")]
/// async fn data(&self) -> Json<Data> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn pre_guard(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as an **event consumer** — called when an event is emitted
/// on the specified event bus.
///
/// The controller must have an `#[inject]` field for the bus. Request-scoped
/// fields are unavailable because consumers run outside HTTP context.
/// The bus field type must implement the [`EventBus`](r2e_events::EventBus) trait.
///
/// ```ignore
/// #[controller(state = Services)]
/// pub struct UserEventConsumer {
///     #[inject] event_bus: LocalEventBus,
/// }
///
/// #[routes]
/// impl UserEventConsumer {
///     #[consumer(bus = "event_bus")]
///     async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
///         tracing::info!(user_id = event.user_id, "user created");
///     }
/// }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn consumer(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a **scheduled background task**.
///
/// Request-scoped fields are unavailable because scheduled tasks run outside
/// HTTP context. Register the controller via
/// `.register_controller::<MyJobs>()` and install the scheduler runtime
/// with `.with(Scheduler)` (from `r2e-scheduler`).
///
/// `every` accepts an integer (seconds) or a duration string with suffixes
/// `ms`, `s`, `m`, `h`, `d` (combinable: `"1h30m"`). Same for `initial_delay`.
/// Cron expressions are validated at compile time.
///
/// ```ignore
/// #[scheduled(every = 30)]                    // every 30 seconds
/// async fn cleanup(&self) { ... }
///
/// #[scheduled(every = "5m")]                  // every 5 minutes
/// async fn sync_data(&self) { ... }
///
/// #[scheduled(every = "2h", initial_delay = "10s")]
/// async fn sync(&self) { ... }
///
/// #[scheduled(cron = "0 */5 * * * *")]        // cron expression
/// async fn report(&self) { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn scheduled(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method on a `#[bean]` impl or a `#[routes]` controller as an
/// **async executor job**.
///
/// The body is moved off the calling thread onto a
/// [`PoolExecutor`](r2e_executor::PoolExecutor) held in a field; calling the
/// method returns `Result<JobHandle<T>, RejectedError>` instead of `T`.
/// Useful for long-running side work (PDF generation, third-party calls,
/// batch jobs) that should not block the caller.
///
/// Requirements:
/// - method is `async fn(&self, ...) -> T`
/// - the bean/controller has a field of type `PoolExecutor`
///   (default field name: `executor`; override with `executor = "name"`)
/// - the bean/controller is `Clone + Send + Sync + 'static`
///   (beans already are; add `#[derive(Clone)]` alongside `#[controller(...)]`).
///
/// ```ignore
/// #[derive(Clone)]
/// pub struct ReportService {
///     executor: PoolExecutor,
/// }
///
/// #[bean]
/// impl ReportService {
///     pub fn new(executor: PoolExecutor) -> Self {
///         Self { executor }
///     }
///
///     #[async_exec]
///     async fn generate_pdf(&self) -> PdfBytes { /* heavy work */ unimplemented!() }
/// }
/// ```
///
/// On a controller, the same attribute works inside the `#[routes]` impl —
/// the controller core is a bean:
///
/// ```ignore
/// #[routes]
/// impl ReportController {
///     #[post("/reports")]
///     async fn create(&self) -> Json<()> {
///         let _job = self.generate_pdf().expect("executor running");
///         Json(())
///     }
///
///     #[async_exec]
///     async fn generate_pdf(&self) -> PdfBytes { /* heavy work */ unimplemented!() }
/// }
/// ```
///
/// This attribute is consumed by [`bean`] / [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn async_exec(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a custom Tower middleware function to a route.
///
/// ```ignore
/// #[middleware(my_middleware_fn)]
/// #[get("/protected")]
/// async fn protected(&self) -> Json<Data> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn middleware(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a Tower layer directly to a single route.
///
/// Unlike [`middleware`] (which wraps a function via `axum::middleware::from_fn`),
/// `#[layer]` takes an arbitrary expression that evaluates to a Tower `Layer`
/// and calls `.layer(expr)` on the route handler.
///
/// ```ignore
/// use tower_http::timeout::TimeoutLayer;
/// use std::time::Duration;
///
/// #[get("/slow")]
/// #[layer(TimeoutLayer::new(Duration::from_secs(2)))]
/// async fn slow_endpoint(&self) -> Json<&'static str> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn layer(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Declare an **SSE** (Server-Sent Events) endpoint.
///
/// The method should return an `impl Stream<Item = Result<SseEvent, Infallible>>`.
/// The macro wraps the result in `Sse::new()` with keep-alive.
///
/// ```ignore
/// #[sse("/events")]
/// async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
///     self.broadcaster.subscribe()
/// }
///
/// #[sse("/events", keep_alive = 15)]   // custom keep-alive interval
/// #[sse("/events", keep_alive = false)] // disable keep-alive
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn sse(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Declare a **WebSocket** endpoint.
///
/// The handler receives a `WsStream` (or raw `WebSocket`) parameter.
/// The macro generates the `WebSocketUpgrade` extraction and `on_upgrade` call.
///
/// ```ignore
/// // Pattern 1: WsStream parameter (recommended)
/// #[ws("/chat")]
/// async fn chat(&self, mut ws: WsStream, Path(room): Path<String>) { ... }
///
/// // Pattern 2: return impl WsHandler (framework manages the loop)
/// #[ws("/echo")]
/// async fn echo(&self) -> impl WsHandler { EchoHandler }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn ws(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Override the default HTTP status code for a route.
///
/// By default, GET/PUT/PATCH return 200, POST returns 201, and DELETE returns 204.
/// Use this attribute to specify a custom status code.
///
/// ```ignore
/// #[post("/users")]
/// #[status(201)]
/// async fn create(&self, body: Json<CreateUser>) -> JsonResult<User> { ... }
///
/// #[delete("/users/{id}")]
/// #[status(200)]
/// async fn delete(&self, Path(id): Path<u64>) -> JsonResult<DeleteResult> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn status(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Specify the response type explicitly for OpenAPI documentation.
///
/// Use this when the return type is an opaque wrapper (e.g., `impl IntoResponse`)
/// and the macro cannot auto-detect the response schema.
///
/// ```ignore
/// #[get("/widgets/{id}")]
/// #[returns(Widget)]
/// async fn get_widget(&self, Path(id): Path<u64>) -> impl IntoResponse { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn returns(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a handler parameter as a **raw Axum extractor** (documentation only).
///
/// This is a no-op marker attribute. All handler parameters that are not
/// annotated with `#[inject(identity)]` or `#[managed]` are already passed
/// as raw Axum extractors. Use `#[raw]` to make this intent explicit:
///
/// ```ignore
/// #[get("/")]
/// async fn handler(
///     &self,
///     #[raw] connect_info: ConnectInfo<SocketAddr>,
///     #[raw] headers: HeaderMap,
/// ) -> Json<&'static str> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn raw(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a handler parameter as a **managed resource** with automatic lifecycle.
///
/// Managed resources implement [`r2e_core::ManagedResource`] and have their
/// lifecycle (acquire/finalize/abort) handled automatically by the macro. The parameter
/// type must be a mutable reference (`&mut T`).
///
/// The most common use case is database transactions via `Tx<DB>`:
///
/// ```ignore
/// use r2e::r2e_data_sqlx::Tx;
/// use sqlx::Sqlite;
///
/// #[post("/users")]
/// async fn create(
///     &self,
///     body: Json<CreateUser>,
///     #[managed] tx: &mut Tx<'_, Sqlite>,
/// ) -> Result<Json<User>, HttpError> {
///     sqlx::query("INSERT INTO users (name) VALUES (?)")
///         .bind(&body.name)
///         .execute(tx.connection())
///         .await?;
///     Ok(Json(user))
/// }
/// ```
///
/// The generated handler will:
/// 1. Call `ManagedResource::acquire()` before the method body
/// 2. Pass `&mut resource` to the method
/// 3. Build the HTTP response and call `ManagedResource::finalize(outcome)`
/// 4. Call `abort()` from a drop guard on panic, cancellation, partial acquire,
///    or failed finalization
///
/// For `Tx<DB>`:
/// - `acquire()` begins a new transaction
/// - responses below status 400 commit the transaction
/// - `4xx`/`5xx` responses roll it back explicitly
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn managed(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

// ---------------------------------------------------------------------------
// Multipart
// ---------------------------------------------------------------------------

/// Derive macro for typed multipart form extraction.
///
/// Generates a `FromMultipart` impl that extracts fields from a
/// `multipart/form-data` request body.
///
/// # Supported field types
///
/// | Rust type | Extraction |
/// |-----------|-----------|
/// | `String` | Required text field |
/// | `UploadedFile` | Required file upload |
/// | `Vec<UploadedFile>` | All files for the field name |
/// | `Bytes` | Raw bytes from either text or file field |
/// | `Option<T>` | Optional version of any above type |
/// | Other (`i32`, `bool`, ...) | Text field parsed via `FromStr` |
///
/// # Example
///
/// ```ignore
/// use r2e::multipart::{FromMultipart, UploadedFile, TypedMultipart};
///
/// #[derive(FromMultipart)]
/// pub struct ProfileUpload {
///     pub name: String,
///     pub bio: Option<String>,
///     pub age: i32,
///     pub avatar: UploadedFile,
///     pub attachments: Vec<UploadedFile>,
/// }
///
/// #[post("/profile")]
/// async fn upload(&self, TypedMultipart(form): TypedMultipart<ProfileUpload>) -> Json<String> {
///     Json(format!("Hello {}, got {} bytes", form.name, form.avatar.len()))
/// }
/// ```
#[proc_macro_derive(FromMultipart)]
pub fn derive_from_multipart(input: TokenStream) -> TokenStream {
    from_multipart::expand(input)
}

// ---------------------------------------------------------------------------
// Bean / DI macros
// ---------------------------------------------------------------------------

/// Mark a method as a **post-construct hook** — called after the bean
/// graph is fully resolved.
///
/// The annotated method must take `&self` and return either `()` or
/// `Result<(), Box<dyn Error + Send + Sync>>`. It may be `async`.
/// Multiple `#[post_construct]` methods are called in declaration order.
///
/// ```ignore
/// #[bean]
/// impl CveRepository {
///     pub fn new(pool: DbPool) -> Self { Self { pool } }
///
///     #[post_construct]
///     async fn cleanup_stale_runs(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
///         self.fail_stale_runs().await?;
///         Ok(())
///     }
/// }
/// ```
///
/// This attribute is consumed by [`bean`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn post_construct(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a **pre-destroy (disposal) hook** — called during the
/// server's graceful shutdown, the symmetric counterpart of
/// [`post_construct`].
///
/// The annotated method must take `&self` and return either `()` or
/// `Result<(), Box<dyn Error + Send + Sync>>`. It may be `async`. Multiple
/// `#[pre_destroy]` methods are called in declaration order. An `Err` is
/// **logged and swallowed** — disposal never aborts shutdown.
///
/// Works on `#[bean]` impls (via [`PreDestroy`](r2e_core::PreDestroy), read by
/// value from the resolved graph — a pinned `override_bean` skips the hook) and
/// on `#[routes]` controller impls (run from the core `Arc` at shutdown). It
/// cannot be combined with a route / `#[scheduled]` / `#[consumer]` /
/// `#[async_exec]` / `#[post_construct]` marker on the same method, and takes no
/// parameters.
///
/// ```ignore
/// #[bean]
/// impl ConnectionPool {
///     pub fn new(cfg: PoolConfig) -> Self { /* ... */ }
///
///     #[pre_destroy]
///     async fn close(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
///         self.drain_and_close().await?;
///         Ok(())
///     }
/// }
/// ```
///
/// This attribute is consumed by [`bean`] / [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn pre_destroy(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Attribute macro on an `impl` block — marks the type as a bean and
/// generates a [`Bean`](r2e_core::beans::Bean) trait impl.
///
/// The macro finds the first associated function that returns `Self` (the
/// constructor) and uses its parameter types as dependencies resolved from
/// the [`BeanContext`](r2e_core::beans::BeanContext).
///
/// # Example
///
/// ```ignore
/// #[bean]
/// impl UserService {
///     pub fn new(event_bus: EventBus) -> Self {
///         Self { event_bus, users: Default::default() }
///     }
/// }
/// ```
///
/// The constructor must be an associated function (no `self` receiver) that
/// returns `Self` or the concrete type name.
#[proc_macro_attribute]
pub fn bean(args: TokenStream, input: TokenStream) -> TokenStream {
    bean_attr::expand(args, input)
}

/// Attribute macro on a free function — marks it as a producer and generates
/// a [`Producer`](r2e_core::beans::Producer) trait impl.
///
/// The macro generates a PascalCase struct from the function name
/// (e.g., `create_pool` → `CreatePool`) and implements the `Producer` trait
/// on it, with the function's return type as `Producer::Output`.
///
/// Supports both sync and async functions. Parameters are resolved from the
/// [`BeanContext`](r2e_core::beans::BeanContext) unless annotated with
/// `#[config("key")]`, in which case they are resolved from `R2eConfig`.
///
/// # Example
///
/// ```ignore
/// #[producer]
/// async fn create_pool(#[config("app.db.url")] url: String) -> SqlitePool {
///     SqlitePool::connect(&url).await.unwrap()
/// }
///
/// // Use with the builder:
/// AppBuilder::new()
///     .provide(config)
///     .register::<CreatePool>()   // registers SqlitePool
///     .build_state()
///     .await
/// ```
#[proc_macro_attribute]
pub fn producer(args: TokenStream, input: TokenStream) -> TokenStream {
    producer_attr::expand(args, input)
}

/// Attribute macro on a struct — generates a
/// [`FeatureModule`](r2e_core::module::FeatureModule) impl from a
/// declarative listing of providers, controllers, exports, and imports.
///
/// Every key is optional and defaults to empty. Register the module with
/// `.register_module::<M>()` on the builder — providers join the bean graph,
/// controllers are wired automatically at `build_state()`, and the
/// closed-subgraph encapsulation checks run at compile time.
///
/// # Example
///
/// ```ignore
/// #[module(
///     providers(UserRepo, UserService),
///     controllers(UserController, AdminController),
///     exports(UserService),      // UserRepo stays private to the module
///     imports(DbPool),           // supplied by the app or another module
/// )]
/// pub struct UserModule;
///
/// AppBuilder::new()
///     .provide(db_pool)
///     .register_module::<UserModule>()
///     .build_state()
///     .await
/// ```
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    module_attr::expand(args, input)
}

/// Derive macro for simple beans whose `#[inject]` fields are resolved
/// from the [`BeanContext`](r2e_core::beans::BeanContext).
///
/// Fields annotated with `#[inject]` are pulled from the context.
/// Fields without `#[inject]` use `Default::default()`.
///
/// # Example
///
/// ```ignore
/// #[derive(Clone, Bean)]
/// pub struct OrderService {
///     #[inject] user_service: UserService,
///     #[inject] event_bus: EventBus,
/// }
/// ```
#[proc_macro_derive(Bean, attributes(inject, config, config_section, default))]
pub fn derive_bean(input: TokenStream) -> TokenStream {
    bean_derive::expand(input)
}

/// Derive macro for guards/interceptors with bean deps — generates the
/// [`DecoratorSpec`](r2e_core::DecoratorSpec) plumbing so the type can be
/// used in `#[guard(...)]` / `#[pre_guard(...)]` / `#[intercept(...)]`
/// without hand-writing a config spec + product pair.
///
/// The derived struct is the finished guard/interceptor (implement
/// `Guard<I>` / `Interceptor<R>` on it). Field attributes:
/// - `#[inject]` — resolved from the bean graph at controller registration
///   (compile-checked, like a controller `#[inject]`)
/// - `#[config("key")]` / `#[config_section(prefix = "...")]` — resolved
///   from `R2eConfig`
/// - plain fields — config set at the attribute site via the generated
///   `Type::spec(...)` constructor (declaration order)
///
/// Caveats: a *misspelled* field attribute is not rejected — the field
/// silently becomes a `spec(...)` constructor argument (the site then fails
/// to compile with an arity/type error rather than an attribute error). And
/// since `spec(...)` shares the struct's visibility, plain-field types must
/// be at least as visible as the struct (E0446 otherwise).
///
/// # Example
///
/// ```ignore
/// #[derive(DecoratorBean)]
/// pub struct DbAuditLog {
///     #[inject] pool: SqlitePool,
///     prefix: String,
/// }
///
/// impl<R: Send> Interceptor<R> for DbAuditLog { /* uses self.pool */ }
///
/// #[intercept(DbAuditLog::spec("api".into()))]
/// async fn create(&self) -> Json<User> { ... }
/// ```
#[proc_macro_derive(DecoratorBean, attributes(inject, config, config_section))]
pub fn derive_decorator_bean(input: TokenStream) -> TokenStream {
    decorator_bean_derive::expand(input)
}

/// Derive macro for background services — generates
/// [`ServiceComponent<State>`](r2e_core::ServiceComponent) so the type can
/// be registered via [`AppBuilder::spawn_service`].
///
/// Field attributes mirror `#[controller(...)]`:
/// - `#[inject]` — clone from app state (type must impl `Clone + Send + Sync`)
/// - `#[config("key")]` — resolve from `R2eConfig`
/// - `#[config_section(prefix = "...")]` — typed config section
///
/// The user supplies an async `run(&self, CancellationToken)` method on the
/// struct; the generated `start` simply forwards to it.
///
/// # Example
///
/// ```ignore
/// use r2e::prelude::*;
/// use tokio_util::sync::CancellationToken;
///
/// #[derive(BackgroundService, Clone)]
/// #[service(state = Services)]
/// pub struct EmailWorker {
///     #[inject] mailer: Mailer,
///     #[inject] executor: PoolExecutor,
///     #[config("email.batch_size")] batch_size: i64,
/// }
///
/// impl EmailWorker {
///     async fn run(&self, shutdown: CancellationToken) { /* loop ... */ }
/// }
///
/// // Register in builder:
/// app.spawn_service::<EmailWorker>();
/// ```
#[proc_macro_derive(BackgroundService, attributes(service, inject, config, config_section))]
pub fn derive_background_service(input: TokenStream) -> TokenStream {
    bg_service_derive::expand(input)
}

/// Derive macro that generates a [`Cacheable`](r2e_core::Cacheable) impl
/// using `serde_json` serialization.
///
/// The type must implement `Serialize` and `DeserializeOwned`.
///
/// # Example
///
/// ```ignore
/// #[derive(Serialize, Deserialize, Cacheable)]
/// pub struct UserList {
///     pub users: Vec<User>,
/// }
/// ```
#[proc_macro_derive(Cacheable)]
pub fn derive_cacheable(input: TokenStream) -> TokenStream {
    cacheable_derive::expand(input)
}

/// Derive macro for strongly-typed configuration sections.
///
/// Generates a [`ConfigProperties`](r2e_core::config::typed::ConfigProperties)
/// impl that maps config keys (under a runtime prefix) to struct fields.
///
/// The prefix is provided at call-site (`from_config(&config, Some("app.database"))`),
/// not on the struct itself.
///
/// # Field attributes
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[config(default = <expr>)]` | Default value if key is missing |
/// | `#[config(key = "nested.key")]` | Override the config key path |
/// | `#[config(env = "VAR")]` | Explicit env var fallback |
/// | `#[config(section)]` | Nested `ConfigProperties` struct |
/// | `Option<T>` field type | Automatically optional (returns `None` if missing) |
/// | No attribute + non-Option | Required — `from_config()` returns error if missing |
///
/// Doc comments on fields become property descriptions in metadata.
///
/// # Example
///
/// ```ignore
/// #[derive(ConfigProperties, Clone, Debug)]
/// pub struct DatabaseConfig {
///     /// Database connection URL
///     pub url: String,
///
///     /// Connection pool size (default: 10)
///     #[config(default = 10)]
///     pub pool_size: i64,
///
///     /// Optional connection timeout in seconds
///     pub timeout: Option<i64>,
/// }
///
/// // Usage:
/// let db_config = DatabaseConfig::from_config(&config, Some("app.database"))?;
/// ```
#[proc_macro_derive(ConfigProperties, attributes(config))]
pub fn derive_config_properties(input: TokenStream) -> TokenStream {
    config_derive::expand(input)
}

/// Derive macro that generates a [`FromConfigValue`] impl via serde deserialization.
///
/// The type must also implement `serde::Deserialize`. This is useful for enums
/// and other types where manual `FromConfigValue` would be tedious.
///
/// # Example
///
/// ```ignore
/// #[derive(serde::Deserialize, FromConfigValue, Clone, Debug)]
/// #[serde(rename_all = "lowercase")]
/// pub enum AppMode {
///     Development,
///     Production,
///     Staging,
/// }
/// ```
///
/// The generated impl delegates to [`deserialize_value`](r2e_core::config::deserialize_value),
/// which converts the `ConfigValue` to JSON and uses serde to deserialize.
#[proc_macro_derive(FromConfigValue)]
pub fn derive_from_config_value(input: TokenStream) -> TokenStream {
    from_config_value_derive::expand(input)
}

// ---------------------------------------------------------------------------
// gRPC support
// ---------------------------------------------------------------------------

/// Attribute macro for wiring a tonic-generated service trait into R2E.
///
/// The first argument is the path to the tonic-generated service trait
/// (e.g., `proto::user_service_server::UserService`). An optional
/// `descriptor = <expr>` argument names the service's encoded
/// `FileDescriptorSet` (`&'static [u8]`, typically
/// `tonic::include_file_descriptor_set!(...)` bytes) so gRPC server
/// reflection (`GrpcServer::with_reflection()`) can describe the service:
///
/// ```ignore
/// #[grpc_routes(proto::user_service_server::UserService, descriptor = proto::FILE_DESCRIPTOR_SET)]
/// ```
///
/// The struct must use [`macro@controller`] (for `#[inject]`, `#[config]`,
/// and the metadata module). The macro generates:
///
/// - A wrapper struct implementing the tonic trait
/// - An `impl GrpcService<State>` for the controller
///
/// # Example
///
/// ```ignore
/// #[controller(state = Services)]
/// pub struct UserGrpcService {
///     #[inject] user_service: UserService,
///     #[config("grpc.max_page_size")] max_page: i64,
/// }
///
/// #[grpc_routes(proto::user_service_server::UserService)]
/// impl UserGrpcService {
///     #[intercept(Logged::info())]
///     async fn get_user(
///         &self,
///         request: Request<GetUserRequest>,
///     ) -> Result<Response<GetUserResponse>, Status> {
///         // ...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn grpc_routes(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(args as grpc_routes_parsing::GrpcRoutesArgs);
    let item = syn::parse_macro_input!(input as syn::ItemImpl);
    match grpc_routes_parsing::parse(args, item) {
        Ok(def) => grpc_codegen::generate(&def).into(),
        Err(err) => err.to_compile_error().into(),
    }
}

// ---------------------------------------------------------------------------
// Params derive
// ---------------------------------------------------------------------------

/// Derive macro for aggregating path, query, and header parameters into a
/// single struct.
///
/// Fields are annotated with `#[path]`, `#[query]`, or `#[header("Name")]`
/// to indicate their extraction source. The generated `FromRequestParts`
/// implementation extracts and parses each field automatically.
///
/// # Attributes
///
/// | Attribute | Source | Default name |
/// |---|---|---|
/// | `#[path]` | URL path segments | field name |
/// | `#[path(name = "userId")]` | URL path segments | custom name |
/// | `#[query]` | Query string | field name |
/// | `#[query(name = "q")]` | Query string | custom name |
/// | `#[header("X-Custom")]` | HTTP headers | explicit name |
///
/// `Option<T>` fields are optional (absent = `None`).
/// Non-Option fields are required (absent = 400 Bad Request).
/// Conversion uses `FromStr` for non-String types.
///
/// # Example
///
/// ```ignore
/// #[derive(Params, garde::Validate)]
/// struct GetUserParams {
///     #[path]
///     id: u64,
///
///     #[query]
///     #[garde(range(min = 1))]
///     page: Option<u32>,
///
///     #[header("X-Tenant-Id")]
///     #[garde(length(min = 1))]
///     tenant_id: String,
/// }
///
/// #[get("/{id}")]
/// async fn get(&self, params: GetUserParams) -> Json<User> {
///     // params.id, params.page, params.tenant_id extracted and validated
/// }
/// ```
#[proc_macro_derive(Params, attributes(path, query, header, param, params))]
pub fn derive_params(input: TokenStream) -> TokenStream {
    params_derive::expand(input)
}

/// Derive macro for ergonomic HTTP error types.
///
/// Generates `impl Display`, `impl IntoResponse`, `impl Error`, and
/// `impl From<T>` (for `#[from]` fields) from a simple enum declaration.
///
/// # Variant attributes
///
/// Each variant **must** have an `#[error(...)]` attribute:
///
/// | Form | Description |
/// |------|-------------|
/// | `#[error(status = NOT_FOUND, message = "...")]` | Explicit status + message |
/// | `#[error(status = NOT_FOUND)]` | Status only — message is inferred |
/// | `#[error(status = 429)]` | Numeric status code |
/// | `#[error(transparent)]` | Delegate to inner type's `IntoResponse` |
///
/// # Field attributes
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[from]` | Generate `From<T>` impl and use source for `Error::source()` |
///
/// # Message inference (when `message` is omitted)
///
/// 1. Single `String` field → uses the field value
/// 2. `#[from]` field → `source.to_string()`
/// 3. Unit variant → humanized name (`AlreadyExists` → `"Already exists"`)
///
/// # Message interpolation
///
/// - `{0}`, `{1}` for tuple fields
/// - `{field_name}` for named fields
///
/// # Example
///
/// ```ignore
/// #[derive(Debug, ApiError)]
/// pub enum MyError {
///     #[error(status = NOT_FOUND, message = "User not found: {0}")]
///     NotFound(String),
///
///     #[error(status = INTERNAL_SERVER_ERROR, message = "Database error")]
///     Database(#[from] sqlx::Error),
///
///     #[error(status = BAD_REQUEST)]
///     Validation(String),           // no message → uses field value
///
///     #[error(status = CONFLICT)]
///     AlreadyExists,                // unit variant → "Already exists"
///
///     #[error(transparent)]
///     Inner(#[from] HttpError),      // delegates to inner IntoResponse
/// }
/// ```
#[proc_macro_derive(ApiError, attributes(error, from))]
pub fn derive_api_error(input: TokenStream) -> TokenStream {
    api_error_derive::expand(input)
}

// ---------------------------------------------------------------------------
// Entry-point macros
// ---------------------------------------------------------------------------

/// Marks an `async fn main()` as the R2E application entry point.
///
/// Wraps the function body in a Tokio multi-thread runtime and calls
/// `init_tracing()` automatically (unless disabled).
///
/// The canonical entry point declares the app via `impl r2e::App` and calls
/// [`r2e::launch`](r2e_core::launch) from a **parameterless** main:
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     r2e::launch::<MyApp>().await.unwrap();
/// }
/// ```
///
/// `main` must take no parameters (the old `setup`-by-convention hot-reload
/// path was removed; hot-reload now lives inside `launch`).
///
/// # Optional arguments
///
/// | Argument | Default | Description |
/// |----------|---------|-------------|
/// | `tracing` | `true` | Call `init_tracing()` before the body |
/// | `flavor` | `"multi_thread"` | Tokio runtime flavor |
/// | `worker_threads` | Tokio default | Number of worker threads |
/// | `max_blocking_threads` | Tokio default (512) | Max threads for blocking tasks |
/// | `thread_stack_size` | Tokio default (2 MiB) | Stack size per worker thread in bytes |
/// | `thread_name` | Tokio default | Worker thread name prefix |
/// | `global_queue_interval` | Tokio default (31) | How often to check the global queue |
/// | `event_interval` | Tokio default (61) | Max events processed per tick |
/// | `thread_keep_alive` | Tokio default (10s) | Keep-alive for blocking threads (seconds) |
/// | `start_paused` | `false` | Start runtime with time paused (useful for tests) |
///
/// # Examples
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     AppBuilder::new()
///         .build_state().await
///         .serve("0.0.0.0:8080").await.unwrap();
/// }
///
/// #[r2e::main(tracing = false)]
/// async fn main() { /* no automatic tracing */ }
///
/// #[r2e::main(flavor = "current_thread")]
/// async fn main() { /* single-threaded runtime */ }
///
/// #[r2e::main(worker_threads = 4)]
/// async fn main() { /* 4 worker threads */ }
///
/// #[r2e::main(thread_stack_size = 8388608, max_blocking_threads = 128)]
/// async fn main() { /* 8 MiB stack, 128 blocking threads */ }
///
/// #[r2e::main(thread_name = "r2e-worker", thread_keep_alive = 30)]
/// async fn main() { /* named threads, 30s keep-alive */ }
/// ```
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    main_attr::expand_main(args, input)
}

/// Marks an `async fn` as an R2E test.
///
/// Wraps the function body in a Tokio **multi-thread** runtime and calls
/// `init_tracing()` automatically.
///
/// # App tests (`app = ...`)
///
/// With `app = <App type>`, the macro boots the [`App`](r2e_core::App)
/// implementation into a `TestApp` (test profile forced, `TestJwt` validators
/// pinned) and binds the test function's parameters from it:
///
/// - `app: TestApp` — the booted app,
/// - `jwt: TestJwt` — the app's auto-wired `TestJwt`,
/// - `#[inject] service: UserService` — any bean from the resolved graph.
///
/// Optional arguments:
///
/// - `with = |b| ...` — builder pre-configuration hook, the place to pin
///   mocks (`b.override_bean(...)`) and patch config
///   (`b.override_config_value(...)`),
/// - `jwt = false` — skip the `TestJwt` auto-wiring (`boot_plain`).
///
/// # Examples
///
/// ```ignore
/// #[r2e::test]
/// async fn test_health() {
///     let app = TestApp::from(build_router().await);
///     let res = app.get("/health").await;
///     assert_eq!(res.status(), 200);
/// }
///
/// #[r2e::test(tracing = false)]
/// async fn test_no_tracing() { /* ... */ }
///
/// #[r2e::test(app = MyApp)]
/// async fn lists_users(app: TestApp) {
///     app.get("/users").as_user("alice", &["user"]).send().await.assert_ok();
/// }
///
/// #[r2e::test(app = MyApp, with = |b| b.override_bean(FakeMailer::new()))]
/// async fn with_mock(app: TestApp, #[inject] mailer: FakeMailer) { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    main_attr::expand_test(args, input)
}

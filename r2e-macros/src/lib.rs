extern crate proc_macro;
use proc_macro::TokenStream;

pub(crate) mod cacheable_derive;
pub(crate) mod config_derive;
pub(crate) mod crate_path;
pub(crate) mod extract;
pub(crate) mod from_multipart;
pub(crate) mod bean_attr;
pub(crate) mod bean_derive;
pub(crate) mod bean_state_derive;
pub(crate) mod producer_attr;
pub(crate) mod derive_codegen;
pub(crate) mod derive_controller;
pub(crate) mod derive_parsing;
pub(crate) mod route;
pub(crate) mod routes_attr;
pub(crate) mod codegen;
pub(crate) mod routes_parsing;
pub(crate) mod types;
pub(crate) mod grpc_codegen;
pub(crate) mod grpc_routes_parsing;

/// Derive macro for declaring a R2E controller struct.
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
/// | `#[identity]` | Request-scoped | **Legacy** — equivalent to `#[inject(identity)]`. |
/// | `#[config("key")]` | App-scoped | Resolved from `R2eConfig` at request time. |
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
/// #[derive(Controller)]
/// #[controller(path = "/users", state = Services)]
/// pub struct UserController {
///     #[inject]  user_service: UserService,
///     #[inject]  pool: sqlx::SqlitePool,
///     #[inject(identity)] user: AuthenticatedUser,
///     #[config("app.greeting")] greeting: String,
/// }
///
/// // Mixed controller (param-level identity)
/// #[derive(Controller)]
/// #[controller(path = "/api", state = Services)]
/// pub struct MixedController {
///     #[inject] user_service: UserService,
///     // No identity on struct → StatefulConstruct is generated
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
/// - A hidden metadata module (`__r2e_meta_<Name>`) — state type alias,
///   path prefix, identity type alias, and identity accessor for guards.
/// - An Axum extractor struct (`__R2eExtract_<Name>`) implementing
///   `FromRequestParts<State>` — constructs the controller from state +
///   request parts.
/// - `impl StatefulConstruct<State> for Name` — **only** when there are no
///   `#[inject(identity)]` fields on the struct. Used by event consumers and
///   scheduled tasks that run outside HTTP context.
#[proc_macro_derive(Controller, attributes(controller, inject, identity, config, config_section))]
pub fn derive_controller(input: TokenStream) -> TokenStream {
    derive_controller::expand(input)
}

/// Attribute macro on an `impl` block — generates Axum handlers, route
/// wiring, and trait impls.
///
/// Must be placed on an `impl` block whose `Self` type derives [`Controller`].
///
/// # Method attributes
///
/// Inside the impl block you can annotate methods with:
///
/// - **HTTP routes**: [`get`], [`post`], [`put`], [`delete`], [`patch`]
/// - **Authorization**: [`roles`]
/// - **Interceptors**: [`intercept`]
/// - **Transactions**: [`transactional`]
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
///     async fn create(&self, body: Validated<CreateUser>) -> Json<User> { ... }
/// }
/// ```
///
/// # What is generated
///
/// - The original `impl` block with method bodies wrapped by interceptors /
///   transactional logic.
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
/// async fn get(&self, Path(id): Path<u64>) -> Result<Json<User>, AppError> { ... }
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
/// async fn create(&self, body: Validated<CreateUser>) -> JsonResult<User> {
///     Ok(Json(self.service.create(body.into_inner()).await?))
/// }
///
/// // Direct type (equivalent)
/// #[post("/users")]
/// async fn create(&self, body: Json<CreateUser>) -> Result<Json<User>, AppError> { ... }
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

/// Wrap a route method body in an automatic SQL transaction.
///
/// The macro injects a `tx` variable (of type `sqlx::Transaction`) into
/// the method body. The transaction is committed on `Ok`, rolled back on
/// `Err`. The return type **must** be `Result<T, AppError>`.
///
/// ```ignore
/// #[post("/users/db")]
/// #[transactional]                       // uses self.pool
/// async fn create_in_db(&self, Json(body): Json<CreateUser>)
///     -> Result<Json<User>, AppError>
/// {
///     sqlx::query("INSERT INTO users (name) VALUES (?)")
///         .bind(&body.name)
///         .execute(&mut *tx)
///         .await?;
///     Ok(Json(user))
/// }
///
/// #[transactional(pool = "read_db")]     // custom pool field
/// async fn read(&self) -> Result<Json<Data>, AppError> { ... }
/// ```
///
/// This attribute is consumed by [`routes`] — it is a no-op on its own.
#[proc_macro_attribute]
pub fn transactional(_args: TokenStream, input: TokenStream) -> TokenStream {
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
/// on the specified `EventBus`.
///
/// The controller must have an `#[inject]` field for the bus, and **must
/// not** have `#[identity]` fields (consumers run outside HTTP context).
///
/// ```ignore
/// #[derive(Controller)]
/// #[controller(state = Services)]
/// pub struct UserEventConsumer {
///     #[inject] event_bus: EventBus,
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
/// The controller **must not** have `#[identity]` fields (scheduled tasks
/// run outside HTTP context). Register the controller via
/// `.register_controller::<MyJobs>()` and install the scheduler runtime
/// with `.with(Scheduler)` (from `r2e-scheduler`).
///
/// ```ignore
/// #[scheduled(every = 30)]                    // every 30 seconds
/// async fn cleanup(&self) { ... }
///
/// #[scheduled(every = 60, delay = 10)]        // first run after 10 s
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
/// lifecycle (acquire/release) handled automatically by the macro. The parameter
/// type must be a mutable reference (`&mut T`).
///
/// The most common use case is database transactions via `Tx<DB>`:
///
/// ```ignore
/// use r2e_core::{Tx, HasPool};
/// use sqlx::Sqlite;
///
/// #[post("/users")]
/// async fn create(
///     &self,
///     body: Json<CreateUser>,
///     #[managed] tx: &mut Tx<Sqlite>,
/// ) -> Result<Json<User>, AppError> {
///     sqlx::query("INSERT INTO users (name) VALUES (?)")
///         .bind(&body.name)
///         .execute(&mut **tx)
///         .await?;
///     Ok(Json(user))
/// }
/// ```
///
/// The generated handler will:
/// 1. Call `ManagedResource::acquire()` before the method body
/// 2. Pass `&mut resource` to the method
/// 3. Call `ManagedResource::release(success)` after the method completes
///    - `success = true` if the method returned `Ok` (or a non-Result type)
///    - `success = false` if the method returned `Err`
///
/// For `Tx<DB>`:
/// - `acquire()` begins a new transaction
/// - `release(true)` commits the transaction
/// - `release(false)` does nothing (transaction rolls back on drop)
///
/// **Note:** `#[managed]` and `#[transactional]` are mutually exclusive.
/// Use `#[managed] tx: &mut Tx<...>` instead of `#[transactional]`.
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
pub fn bean(_args: TokenStream, input: TokenStream) -> TokenStream {
    bean_attr::expand(input)
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
///     .with_producer::<CreatePool>()   // registers SqlitePool
///     .build_state::<Services, _>()
///     .await
/// ```
#[proc_macro_attribute]
pub fn producer(_args: TokenStream, input: TokenStream) -> TokenStream {
    producer_attr::expand(input)
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
#[proc_macro_derive(Bean, attributes(inject, config))]
pub fn derive_bean(input: TokenStream) -> TokenStream {
    bean_derive::expand(input)
}

/// Derive macro for state structs — generates
/// [`BeanState::from_context()`](r2e_core::beans::BeanState) and
/// `FromRef` impls for each field.
///
/// Every field is resolved from the [`BeanContext`] by type. If two fields
/// share the same type, `FromRef` is generated only for the first one.
/// Use `#[bean_state(skip_from_ref)]` on a field to suppress its `FromRef`
/// impl.
///
/// # Example
///
/// ```ignore
/// #[derive(Clone, BeanState)]
/// pub struct Services {
///     pub user_service: UserService,
///     pub event_bus: EventBus,
///     pub pool: SqlitePool,
/// }
/// ```
#[proc_macro_derive(BeanState, attributes(bean_state))]
pub fn derive_bean_state(input: TokenStream) -> TokenStream {
    bean_state_derive::expand(input)
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
/// impl that maps a YAML config prefix to struct fields.
///
/// # Struct-level attribute
///
/// `#[config(prefix = "...")]` sets the config key prefix.
///
/// # Field attributes
///
/// | Attribute | Description |
/// |-----------|-------------|
/// | `#[config(default = <expr>)]` | Default value if key is missing |
/// | `Option<T>` field type | Automatically optional (returns `None` if missing) |
/// | No attribute + non-Option | Required — `from_config()` returns error if missing |
///
/// Doc comments on fields become property descriptions in metadata.
///
/// # Example
///
/// ```ignore
/// #[derive(ConfigProperties, Clone, Debug)]
/// #[config(prefix = "app.database")]
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
/// let db_config = DatabaseConfig::from_config(&config)?;
/// ```
#[proc_macro_derive(ConfigProperties, attributes(config))]
pub fn derive_config_properties(input: TokenStream) -> TokenStream {
    config_derive::expand(input)
}

// ---------------------------------------------------------------------------
// gRPC support
// ---------------------------------------------------------------------------

/// Attribute macro for wiring a tonic-generated service trait into R2E.
///
/// The argument is the path to the tonic-generated service trait
/// (e.g., `proto::user_service_server::UserService`).
///
/// The struct must derive [`Controller`] (for `#[inject]`, `#[config]`,
/// and the metadata module). The macro generates:
///
/// - A wrapper struct implementing the tonic trait
/// - An `impl GrpcService<State>` for the controller
///
/// # Example
///
/// ```ignore
/// #[derive(Controller)]
/// #[controller(state = Services)]
/// pub struct UserGrpcService {
///     #[inject] user_service: UserService,
///     #[config("grpc.max_page_size")] max_page: i64,
/// }
///
/// #[grpc_routes(proto::user_service_server::UserService)]
/// impl UserGrpcService {
///     #[guard(AdminGuard)]
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
    let service_trait = syn::parse_macro_input!(args as syn::Path);
    let item = syn::parse_macro_input!(input as syn::ItemImpl);
    match grpc_routes_parsing::parse(service_trait, item) {
        Ok(def) => grpc_codegen::generate(&def).into(),
        Err(err) => err.to_compile_error().into(),
    }
}

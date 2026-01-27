extern crate proc_macro;
use proc_macro::TokenStream;

pub(crate) mod codegen;
pub(crate) mod controller;
pub(crate) mod parsing;
pub(crate) mod route;

/// Declare a controller with automatic Axum handler generation.
///
/// ```ignore
/// quarlus_macros::controller! {
///     state = Services;
///
///     impl UserResource {
///         #[inject]
///         user_service: UserService,
///
///         #[identity]
///         user: AuthenticatedUser,
///
///         #[get("/users")]
///         async fn list(&self) -> Json<Vec<User>> {
///             Json(self.user_service.list().await)
///         }
///     }
/// }
/// ```
///
/// Generates:
/// - The struct definition with inject + identity fields
/// - An impl block with the original methods
/// - Free handler functions for Axum
/// - `impl Controller<T>` with `fn routes()`
#[proc_macro]
pub fn controller(input: TokenStream) -> TokenStream {
    controller::expand(input)
}

/// Mark a method as a GET route handler.
/// No-op attribute — read by `controller!`.
#[proc_macro_attribute]
pub fn get(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a POST route handler.
#[proc_macro_attribute]
pub fn post(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a PUT route handler.
#[proc_macro_attribute]
pub fn put(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a DELETE route handler.
#[proc_macro_attribute]
pub fn delete(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a PATCH route handler.
#[proc_macro_attribute]
pub fn patch(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a field as app-scoped (injected from AppState).
/// No-op attribute — read by `controller!`.
#[proc_macro_attribute]
pub fn inject(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a field as request-scoped (extracted from the HTTP request).
/// No-op attribute — read by `controller!`.
#[proc_macro_attribute]
pub fn identity(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Restrict a route to users that have at least one of the specified roles.
/// No-op attribute — read by `controller!`.
///
/// ```ignore
/// #[get("/admin/users")]
/// #[roles("admin")]
/// async fn admin_list(&self) -> Json<Vec<User>> { ... }
/// ```
#[proc_macro_attribute]
pub fn roles(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Wrap a route method body in an automatic SQL transaction.
/// The macro injects a `tx` variable (begun from `self.pool`) and
/// commits on `Ok`, rolls back on `Err` (via drop).
/// No-op attribute — read by `controller!`.
#[proc_macro_attribute]
pub fn transactional(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Inject a configuration value into a controller field.
/// No-op attribute — read by `controller!`.
///
/// ```ignore
/// #[config("app.greeting")]
/// greeting: String,
/// ```
#[proc_macro_attribute]
pub fn config(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Rate-limit a route method by IP or a static key.
/// No-op attribute — read by `controller!`.
///
/// ```ignore
/// #[rate_limited(max = 100, window = 60)]
/// async fn create(&self, ...) -> ... { ... }
/// ```
#[proc_macro_attribute]
pub fn rate_limited(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply an interceptor to a route method.
/// The expression must evaluate to a type implementing `quarlus_core::Interceptor<R>`.
/// No-op attribute — read by `controller!`.
///
/// Accepts any expression: unit structs, method calls, chained builders.
///
/// ```ignore
/// #[intercept(AuditLog)]
/// #[intercept(Logged::debug())]
/// #[intercept(Cache::ttl(30).group("users"))]
/// ```
#[proc_macro_attribute]
pub fn intercept(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as an event consumer.
/// No-op attribute — read by `controller!`.
///
/// The method's parameter must be `Arc<EventType>`. The event type is inferred
/// from the parameter. Controllers with `#[consumer]` methods **cannot** have
/// `#[identity]` fields (no HTTP request context is available for consumers).
///
/// ```ignore
/// #[consumer(bus = "event_bus")]
/// async fn on_user_created(&self, event: Arc<UserCreatedEvent>) { ... }
/// ```
#[proc_macro_attribute]
pub fn consumer(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Set a base path prefix for all routes in this controller.
/// Uses `axum::Router::nest()` under the hood.
/// No-op attribute — read by `controller!`.
///
/// ```ignore
/// controller! {
///     #[path("/users")]
///     impl UserController for Services {
///         #[get("/")]
///         async fn list(&self) -> Json<Vec<User>> { ... }
///
///         #[get("/{id}")]
///         async fn get_by_id(&self, Path(id): Path<u64>) -> ... { ... }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn path(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a custom middleware function to a route.
/// No-op attribute — read by `controller!`.
///
/// ```ignore
/// #[middleware(my_middleware_fn)]
/// #[get("/protected")]
/// async fn protected(&self) -> ... { ... }
/// ```
#[proc_macro_attribute]
pub fn middleware(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

extern crate proc_macro;
use proc_macro::TokenStream;

pub(crate) mod attr_extract;
pub(crate) mod derive_codegen;
pub(crate) mod derive_controller;
pub(crate) mod derive_parsing;
pub(crate) mod route;
pub(crate) mod routes_attr;
pub(crate) mod routes_codegen;
pub(crate) mod routes_parsing;
pub(crate) mod types;

/// Derive macro that generates the controller struct metadata, an Axum
/// extractor, and (when applicable) a `StatefulConstruct` impl.
///
/// ```ignore
/// #[derive(Controller)]
/// #[controller(path = "/users", state = Services)]
/// pub struct UserController {
///     #[inject]  user_service: UserService,
///     #[identity] user: AuthenticatedUser,
///     #[config("app.greeting")] greeting: String,
/// }
/// ```
///
/// Generates (hidden):
/// - `mod __quarlus_meta_<Name>` — type alias for State, PATH_PREFIX, guard_identity
/// - `struct __QuarlusExtract_<Name>` — `FromRequestParts` extractor
/// - `impl StatefulConstruct<State> for Name` — only when no `#[identity]` fields
#[proc_macro_derive(Controller, attributes(controller, inject, identity, config))]
pub fn derive_controller(input: TokenStream) -> TokenStream {
    derive_controller::expand(input)
}

/// Attribute macro on an `impl` block that generates Axum handlers, route
/// registration, and trait impls (`Controller`, `ScheduledController`).
///
/// Must be paired with a struct that derives `Controller`.
///
/// ```ignore
/// #[routes]
/// #[intercept(Logged::info())]
/// impl UserController {
///     #[get("/")]
///     async fn list(&self) -> Json<Vec<User>> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn routes(_args: TokenStream, input: TokenStream) -> TokenStream {
    routes_attr::expand(input)
}

// ---------------------------------------------------------------------------
// No-op attributes — consumed by #[routes] from the token stream.
// Declared here for IDE support (rust-analyzer), cargo doc, and to prevent
// "cannot find attribute" errors when used outside #[routes].
// ---------------------------------------------------------------------------

/// Mark a method as a GET route handler.
/// No-op attribute — read by `#[routes]`.
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

/// Restrict a route to users that have at least one of the specified roles.
/// No-op attribute — read by `#[routes]`.
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
/// No-op attribute — read by `#[routes]`.
#[proc_macro_attribute]
pub fn transactional(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Rate-limit a route method by IP or a static key.
/// No-op attribute — read by `#[routes]`.
///
/// ```ignore
/// #[rate_limited(max = 100, window = 60)]
/// async fn create(&self, ...) -> ... { ... }
/// ```
#[proc_macro_attribute]
pub fn rate_limited(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply an interceptor to a method or impl block.
/// No-op attribute — read by `#[routes]`.
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

/// Apply a guard to a route method.
/// No-op attribute — read by `#[routes]`.
///
/// ```ignore
/// #[guard(MyCustomGuard)]
/// #[get("/protected")]
/// async fn protected(&self) -> ... { ... }
/// ```
#[proc_macro_attribute]
pub fn guard(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as an event consumer.
/// No-op attribute — read by `#[routes]`.
///
/// ```ignore
/// #[consumer(bus = "event_bus")]
/// async fn on_user_created(&self, event: Arc<UserCreatedEvent>) { ... }
/// ```
#[proc_macro_attribute]
pub fn consumer(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Mark a method as a scheduled background task.
/// No-op attribute — read by `#[routes]`.
///
/// ```ignore
/// #[scheduled(every = 30)]
/// #[scheduled(cron = "0 */5 * * * *")]
/// ```
#[proc_macro_attribute]
pub fn scheduled(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Apply a custom middleware function to a route.
/// No-op attribute — read by `#[routes]`.
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

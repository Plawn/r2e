pub use axum::routing::{any, delete, get, patch, post, put, MethodRouter, Route};

/// Whether `router` carries a user-installed fallback — [`Router::fallback`]
/// / [`Router::fallback_service`] on it, or merged into it from a router that
/// had one — as opposed to axum's built-in empty 404.
///
/// axum keeps this bit privately (it is what makes `merge` panic on two
/// custom fallbacks) and exposes it only through `Router`'s `Debug` output,
/// as the `default_fallback` field. The field is printed after the two
/// path routers and before `catch_all_fallback` (whose own `Debug` carries no
/// user data), so the **last** occurrence is always the top-level one: a route
/// path that happened to spell the field name could only appear earlier.
/// Pinned to the axum version this crate owns; `tests/routing.rs` covers both
/// states across `with_state`, `layer`, `merge` and `nest`.
///
/// [`Router::fallback`]: crate::Router::fallback
/// [`Router::fallback_service`]: crate::Router::fallback_service
#[must_use]
pub fn has_custom_fallback<S>(router: &crate::Router<S>) -> bool {
    const FIELD: &str = ", default_fallback: ";
    let repr = format!("{router:?}");
    match repr.rfind(FIELD) {
        Some(at) => repr[at + FIELD.len()..].starts_with("false"),
        None => false,
    }
}

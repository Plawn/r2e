//! Escape hatch: the raw `axum` API, reachable on purpose.
//!
//! **Importing from here couples your application to the axum backend.** The
//! supported public surface of R2E is the R2E types under `r2e::http` (and the
//! subset of them in `r2e::prelude`); R2E's own contracts —
//! `r2e::web::extract::FromRequestPartsVia` for extraction,
//! [`IntoHttpResponse`](crate::response::IntoHttpResponse) for responses — are
//! what R2E promises to keep working across an HTTP-backend change. Nothing
//! here is covered by that promise.
//!
//! It exists because a re-export shim can never be complete: tower layers with
//! axum-typed bounds, `axum::debug_handler`, a third-party crate whose API is
//! spelled in axum types, an extractor R2E has not re-exported. Reaching for
//! axum directly is a legitimate answer to those — the point of this module is
//! that it be **explicit and greppable** (`rg axum_compat`) rather than a
//! `Cargo.toml` line nobody revisits.
//!
//! ```ignore
//! // the whole crate, under its own name
//! use r2e::http::axum_compat::axum;
//!
//! // or an item directly
//! use r2e::http::axum_compat::routing::method_routing;
//! ```
//!
//! If what you need turns out to be broadly useful, prefer opening an issue to
//! have it re-exported from `r2e::http` over spreading `axum_compat` imports.
//!
//! See `plans/runtime-http-dependency-containment.md` §5.3d (decision A,
//! 2026-08-24).

/// The `axum` crate itself, under its own name.
pub use axum;

pub use axum::*;

/// The backend router behind R2E's [`Routes`](crate::Routes).
///
/// For merging a hand-built or third-party `axum::Router` into an R2E app
/// (`AppBuilder::merge_router(from_router(..))`) or handing R2E's routes to
/// something spelled in axum types. The usual caveat applies: this couples
/// the call site to the backend.
pub fn into_router<S>(routes: crate::Routes<S>) -> axum::Router<S> {
    routes.into_inner()
}

/// R2E [`Routes`](crate::Routes) over a backend router. See [`into_router`].
pub fn from_router<S>(router: axum::Router<S>) -> crate::Routes<S> {
    crate::Routes::from_inner(router)
}

/// The backend method router behind R2E's [`MethodRoutes`](crate::MethodRoutes).
pub fn into_method_router<S>(
    routes: crate::MethodRoutes<S>,
) -> axum::routing::MethodRouter<S, std::convert::Infallible>
where
    S: Clone + Send + Sync + 'static,
{
    routes.into_inner()
}

/// R2E [`MethodRoutes`](crate::MethodRoutes) over a backend method router —
/// the way to mount a handler written against axum's extractor tuple.
pub fn from_method_router<S>(
    router: axum::routing::MethodRouter<S, std::convert::Infallible>,
) -> crate::MethodRoutes<S>
where
    S: Clone + Send + Sync + 'static,
{
    crate::MethodRoutes::from_inner(router)
}

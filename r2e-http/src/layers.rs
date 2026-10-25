//! Tower-http layers behind an R2E name.
//!
//! `tower-http` is the HTTP backend's middleware companion. It is confined to
//! this crate — plus `r2e-grpc`'s grpc-web arm, which fronts tonic rather than
//! the HTTP router — so a backend change has one place to re-source these
//! from. The types are tower-http's own: identity is preserved, a
//! [`cors::CorsLayer`] built here is the very type `tower-http` hands out, and
//! the two interoperate freely.
//!
//! `tower` itself is deliberately **not** behind this module. `Layer` /
//! `Service` are the neutral middleware vocabulary, like `http` and `bytes`,
//! and stay public (plans/axum-containment-roadmap.md, decision D1).
//!
//! ```rust,ignore
//! use r2e::http::layers::timeout::TimeoutLayer;
//! use std::time::Duration;
//!
//! #[get("/slow")]
//! #[layer(TimeoutLayer::new(Duration::from_secs(2)))]
//! async fn slow(&self) -> Json<&'static str> { Json("done") }
//! ```

use crate::Routes;

pub use tower_http::cors;
pub use tower_http::normalize_path;
pub use tower_http::set_header;
pub use tower_http::timeout;

/// A permissive CORS layer: any origin, any method, any header.
///
/// Suitable for development or internal services. For production, build a
/// stricter [`cors::CorsLayer`] and hand it to the `Cors::custom(..)` plugin.
pub fn default_cors() -> cors::CorsLayer {
    cors::CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods(cors::Any)
        .allow_headers(cors::Any)
}

/// Wrap fully-built routes in pre-routing trailing-slash normalization.
///
/// A middleware added via `Routes::layer` runs after routing and cannot
/// change which route matches, so the rewrite must wrap the routes
/// themselves: the whole tree goes inside tower-http's `NormalizePath`
/// service, and the wrapped service is re-embedded as the fallback of a fresh
/// empty `Routes` so callers keep receiving plain `Routes`. The outer tree is
/// a trivial extra dispatch hop — the meaningful routing (and `MatchedPath`
/// insertion) happens once, in the wrapped inner tree. Anything applied
/// OUTSIDE this wrap therefore never sees `MatchedPath`.
///
/// tower-http's `trim_trailing_slash` also collapses a leading run of
/// slashes (`//admin` → `/admin`).
pub fn normalize_path_router(routes: Routes) -> Routes {
    use tower::Layer as _;
    let svc = normalize_path::NormalizePathLayer::trim_trailing_slash().layer(routes);
    Routes::new().fallback_service(svc)
}

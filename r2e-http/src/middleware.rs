//! Function middleware: the [`Next`] handed to an
//! `async fn(Request, Next) -> Response`.
//!
//! Middleware functions are attached with
//! [`Routes::middleware_fn`](crate::routing::Routes::middleware_fn) /
//! [`MethodRoutes::middleware_fn`](crate::routing::MethodRoutes::middleware_fn)
//! (and their `_with_state` forms); the controller codegen emits those for
//! `#[middleware(f)]` / `#[middleware(f, state)]`.
//!
//! ```rust
//! use r2e_http::middleware::Next;
//! use r2e_http::routing::{get, Routes};
//! use r2e_http::{Request, Response};
//!
//! async fn tag(req: Request, next: Next) -> Response {
//!     let mut res = next.run(req).await;
//!     res.headers_mut().insert("x-tagged", "1".parse().unwrap());
//!     res
//! }
//!
//! let routes: Routes = Routes::new()
//!     .route("/", get(|_: (), _req| async { "hi" }))
//!     .middleware_fn(tag);
//! # let _ = routes;
//! ```

use crate::{Request, Response};

/// The rest of the middleware stack, down to the handler.
///
/// Consumed by [`run`](Self::run); a middleware that does not call it
/// short-circuits the request.
#[derive(Debug, Clone)]
pub struct Next {
    inner: axum::middleware::Next,
}

impl Next {
    pub(crate) fn new(inner: axum::middleware::Next) -> Self {
        Self { inner }
    }

    /// Run the remaining middleware and the handler.
    pub async fn run(self, req: Request) -> Response {
        self.inner.run(req).await
    }
}

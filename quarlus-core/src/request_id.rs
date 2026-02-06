//! Request ID middleware — propagates or generates a unique identifier per request.
//!
//! # Behavior
//!
//! 1. Reads `X-Request-Id` from the incoming request headers; if absent, generates a UUID v4.
//! 2. Stores the ID as an Axum request extension (extractable in handlers).
//! 3. Copies the ID into the response `X-Request-Id` header.
//!
//! # Usage
//!
//! ```ignore
//! use quarlus_core::RequestId;
//!
//! // As a plugin
//! AppBuilder::new()
//!     .build_state::<S, _>()
//!     .with(RequestId)
//!     // ...
//!
//! // As an extractor in handlers
//! #[get("/")]
//! async fn handler(&self, req_id: RequestId) -> String {
//!     format!("request: {}", req_id)
//! }
//! ```

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};

use crate::builder::AppBuilder;
use crate::plugin::Plugin;

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// A request identifier — either propagated from the incoming `X-Request-Id` header
/// or generated as a UUID v4.
///
/// Implements [`FromRequestParts`] for use as a handler parameter and [`Display`]
/// for logging.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let id = parts
                .extensions
                .get::<RequestId>()
                .cloned()
                .unwrap_or_else(|| RequestId(uuid::Uuid::new_v4().to_string()));
            Ok(id)
        }
    }
}

impl IntoResponse for RequestId {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}

/// Middleware function that injects the request ID.
async fn request_id_middleware(
    mut req: axum::extract::Request,
    next: crate::http::middleware::Next,
) -> Response {
    let id = req
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let request_id = RequestId(id);
    req.extensions_mut().insert(request_id.clone());

    let mut response = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert(X_REQUEST_ID.clone(), val);
    }

    response
}

/// Plugin that installs the Request ID middleware.
///
/// ```ignore
/// .with(RequestIdPlugin)
/// ```
pub struct RequestIdPlugin;

impl Plugin for RequestIdPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| {
            router.layer(axum::middleware::from_fn(request_id_middleware))
        })
    }
}

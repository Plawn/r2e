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
//! use r2e_core::RequestId;
//!
//! // As a plugin
//! AppBuilder::new()
//!     .build_state::<S, _, _>()
//!     .await
//!     .with(RequestId)
//!     // ...
//!
//! // As an extractor in handlers
//! #[get("/")]
//! async fn handler(&self, req_id: RequestId) -> String {
//!     format!("request: {}", req_id)
//! }
//! ```

use crate::http::extract::FromRequestParts;
use crate::http::header::Parts;
use crate::http::{HeaderName, HeaderValue};
use crate::http::response::{IntoResponse, Response};

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
    mut req: crate::http::Request,
    next: crate::http::middleware::Next,
) -> Response {
    // Build (id_string, header_value) once per request, avoiding the
    // double alloc (String + HeaderValue) of the naive path.
    let (id, header_val) = if let Some(v) = req.headers().get(&X_REQUEST_ID) {
        match v.to_str() {
            Ok(s) => (s.to_string(), v.clone()),
            Err(_) => fresh_request_id(),
        }
    } else {
        fresh_request_id()
    };

    req.extensions_mut().insert(RequestId(id));

    let mut response = next.run(req).await;
    response.headers_mut().insert(X_REQUEST_ID.clone(), header_val);
    response
}

/// Generate a fresh UUID v4 into a stack buffer and build the matching
/// `HeaderValue` without paying for `HeaderValue::from_str`'s validation —
/// the hyphenated UUID encoding is always valid visible ASCII.
fn fresh_request_id() -> (String, HeaderValue) {
    let mut buf = [0u8; uuid::fmt::Hyphenated::LENGTH];
    let encoded = uuid::Uuid::new_v4().as_hyphenated().encode_lower(&mut buf);
    // Safety note: `encode_lower` writes only `[0-9a-f-]`, which is valid
    // UTF-8 and valid HeaderValue content. `from_bytes` is infallible here.
    let header_val = HeaderValue::from_bytes(encoded.as_bytes())
        .expect("hyphenated UUID is always a valid header value");
    (encoded.to_owned(), header_val)
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
            router.layer(crate::http::middleware::from_fn(request_id_middleware))
        })
    }
}

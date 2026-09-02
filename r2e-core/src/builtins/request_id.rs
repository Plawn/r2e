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
//!     .plugin(RequestIdPlugin)
//!     .build_state()
//!     .await
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
use crate::http::response::{IntoHttpResponse, IntoResponse, Response};
use crate::http::{HeaderName, HeaderValue};

use crate::plugin::Plugin;

static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// A request identifier — either propagated from the incoming `X-Request-Id` header
/// or generated as a UUID v4.
///
/// Implements [`FromRequestParts`] for use as a handler parameter and [`Display`]
/// for logging. That impl is a named bridge point (plan §5.3b): route-method
/// parameters are extracted by the HTTP backend, not through
/// `FromRequestPartsVia`. The response side went the other way — see the
/// [`IntoHttpResponse`] impl below.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let id = parts
            .extensions
            .get::<RequestId>()
            .cloned()
            .unwrap_or_else(|| RequestId(uuid::Uuid::new_v4().to_string()));
        Ok(id)
    }
}

impl IntoHttpResponse for RequestId {
    fn into_http_response(self) -> Response {
        self.0.into_response()
    }
}

crate::http::impl_into_response!(RequestId);

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
    response
        .headers_mut()
        .insert(X_REQUEST_ID.clone(), header_val);
    response
}

/// Generate a fresh UUID v4 into a stack buffer and build the matching
/// `HeaderValue` without paying for `HeaderValue::from_str`'s validation —
/// the hyphenated UUID encoding is always valid visible ASCII.
///
/// Shared with the [`HttpTrace`](crate::builtins::HttpTrace) layer, which mints
/// the same shape of id when no `RequestId` extension and no inbound
/// `x-request-id` header resolved one.
pub(crate) fn fresh_request_id() -> (String, HeaderValue) {
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
/// .plugin(RequestIdPlugin)
/// ```
///
/// [`HttpTrace`](crate::builtins::HttpTrace) **includes this behaviour**
/// (`trace.request-id`, on by default): it resolves or mints the id, publishes
/// it as the `RequestId` extension *and* the inbound `x-request-id` header, and
/// echoes it on the response. Installing both plugins in either order is
/// harmless — they agree on one id per request — so reach for this one only
/// when you want request ids **without** request tracing.
pub struct RequestIdPlugin;

impl Plugin for RequestIdPlugin {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut crate::plugin::PluginBuildContext,
    ) -> Result<Self::Provided, crate::plugin::PluginBuildError> {
        ctx.add_layer(|router| {
            router.layer(crate::http::middleware::from_fn(request_id_middleware))
        });
        Ok(())
    }
}

use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

/// Helper to create a JSON error response with a standard `{ "error": message }` body.
pub fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": message.into() });
    (status, Json(body)).into_response()
}

pub enum HttpError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Internal(String),
    Validation(crate::validation::ValidationErrorResponse),
    Custom {
        status: StatusCode,
        body: serde_json::Value,
    },
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        match self {
            HttpError::Validation(resp) => {
                let body = serde_json::json!({
                    "error": "Validation failed",
                    "details": resp.errors,
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            HttpError::Custom { status, body } => {
                (status, Json(body)).into_response()
            }
            other => {
                let (status, message) = match other {
                    HttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
                    HttpError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    HttpError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    HttpError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
                    HttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                    HttpError::Validation(_) => unreachable!(),
                    HttpError::Custom { .. } => unreachable!(),
                };
                error_response(status, message)
            }
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::NotFound(msg) => write!(f, "Not Found: {msg}"),
            HttpError::Unauthorized(msg) => write!(f, "Unauthorized: {msg}"),
            HttpError::Forbidden(msg) => write!(f, "Forbidden: {msg}"),
            HttpError::BadRequest(msg) => write!(f, "Bad Request: {msg}"),
            HttpError::Internal(msg) => write!(f, "Internal Error: {msg}"),
            HttpError::Validation(resp) => write!(f, "Validation Error: {} errors", resp.errors.len()),
            HttpError::Custom { status, body } => write!(f, "Custom Error ({status}): {body}"),
        }
    }
}

impl std::fmt::Debug for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Display>::fmt(self, f)
    }
}

impl From<std::io::Error> for HttpError {
    fn from(err: std::io::Error) -> Self {
        HttpError::Internal(err.to_string())
    }
}

/// Generate `From<E> for HttpError` implementations that map error types to
/// a specific `HttpError` variant.
///
/// # Example
///
/// ```ignore
/// r2e_core::map_error! {
///     sqlx::Error => Internal,
///     std::io::Error => Internal,
/// }
/// ```
#[macro_export]
macro_rules! map_error {
    ( $( $err_ty:ty => $variant:ident ),* $(,)? ) => {
        $(
            impl From<$err_ty> for $crate::HttpError {
                fn from(err: $err_ty) -> Self {
                    $crate::HttpError::$variant(err.to_string())
                }
            }
        )*
    };
}

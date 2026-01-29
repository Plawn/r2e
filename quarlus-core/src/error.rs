use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

pub enum AppError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Internal(String),
    #[cfg(feature = "validation")]
    Validation(crate::validation::ValidationErrorResponse),
    Custom {
        status: StatusCode,
        body: serde_json::Value,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            #[cfg(feature = "validation")]
            AppError::Validation(resp) => {
                let body = serde_json::json!({
                    "error": "Validation failed",
                    "details": resp.errors,
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            AppError::Custom { status, body } => {
                (status, Json(body)).into_response()
            }
            other => {
                let (status, message) = match other {
                    AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
                    AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
                    AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                    #[cfg(feature = "validation")]
                    AppError::Validation(_) => unreachable!(),
                    AppError::Custom { .. } => unreachable!(),
                };
                let body = serde_json::json!({ "error": message });
                (status, Json(body)).into_response()
            }
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::NotFound(msg) => write!(f, "Not Found: {msg}"),
            AppError::Unauthorized(msg) => write!(f, "Unauthorized: {msg}"),
            AppError::Forbidden(msg) => write!(f, "Forbidden: {msg}"),
            AppError::BadRequest(msg) => write!(f, "Bad Request: {msg}"),
            AppError::Internal(msg) => write!(f, "Internal Error: {msg}"),
            #[cfg(feature = "validation")]
            AppError::Validation(resp) => write!(f, "Validation Error: {} errors", resp.errors.len()),
            AppError::Custom { status, body } => write!(f, "Custom Error ({status}): {body}"),
        }
    }
}

impl std::fmt::Debug for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Display>::fmt(self, f)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

/// Generate `From<E> for AppError` implementations that map error types to
/// a specific `AppError` variant.
///
/// # Example
///
/// ```ignore
/// quarlus_core::map_error! {
///     sqlx::Error => Internal,
///     std::io::Error => Internal,
/// }
/// ```
#[macro_export]
macro_rules! map_error {
    ( $( $err_ty:ty => $variant:ident ),* $(,)? ) => {
        $(
            impl From<$err_ty> for $crate::AppError {
                fn from(err: $err_ty) -> Self {
                    $crate::AppError::$variant(err.to_string())
                }
            }
        )*
    };
}

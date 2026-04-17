use std::borrow::Cow;
use std::sync::Arc;

use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

// ── Efficient error body serialization ────────────────────────────────

#[derive(serde::Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

/// Helper to create a JSON error response with a standard `{ "error": message }` body.
pub fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let message = message.into();
    let body = ErrorBody { error: &message };
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| {
        br#"{"error":"internal serialization error"}"#.to_vec()
    });
    (
        status,
        [(
            crate::http::header::CONTENT_TYPE,
            crate::http::HeaderValue::from_static("application/json"),
        )],
        bytes,
    )
        .into_response()
}

// ── HttpError ─────────────────────────────────────────────────────────

#[non_exhaustive]
pub enum HttpError {
    NotFound(Cow<'static, str>),
    Unauthorized(Cow<'static, str>),
    Forbidden(Cow<'static, str>),
    BadRequest(Cow<'static, str>),
    Internal(Cow<'static, str>),
    Validation(crate::validation::ValidationErrorResponse),
    Custom {
        status: StatusCode,
        body: serde_json::Value,
    },
    /// Error variant that preserves the source error chain.
    ///
    /// Used by `From` conversions (e.g., `From<DataError>`) to keep the
    /// original error accessible via `std::error::Error::source()`.
    /// The `source` is never exposed to the HTTP client — only `message`.
    ///
    /// `source` is wrapped in `Arc` so `Clone` is cheap and preserves the
    /// original concrete error type (downcasting through `source()` still works).
    WithSource {
        status: StatusCode,
        message: Cow<'static, str>,
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
}

impl Clone for HttpError {
    fn clone(&self) -> Self {
        match self {
            HttpError::NotFound(msg) => HttpError::NotFound(msg.clone()),
            HttpError::Unauthorized(msg) => HttpError::Unauthorized(msg.clone()),
            HttpError::Forbidden(msg) => HttpError::Forbidden(msg.clone()),
            HttpError::BadRequest(msg) => HttpError::BadRequest(msg.clone()),
            HttpError::Internal(msg) => HttpError::Internal(msg.clone()),
            HttpError::Validation(resp) => HttpError::Validation(resp.clone()),
            HttpError::Custom { status, body } => HttpError::Custom {
                status: *status,
                body: body.clone(),
            },
            HttpError::WithSource {
                status, message, source,
            } => HttpError::WithSource {
                status: *status,
                message: message.clone(),
                source: Arc::clone(source),
            },
        }
    }
}

// ── Convenience constructors ──────────────────────────────────────────

impl HttpError {
    /// Create an error from a status code and message.
    pub fn from_status(status: StatusCode, message: impl Into<Cow<'static, str>>) -> Self {
        let message = message.into();
        match status {
            StatusCode::NOT_FOUND => HttpError::NotFound(message),
            StatusCode::UNAUTHORIZED => HttpError::Unauthorized(message),
            StatusCode::FORBIDDEN => HttpError::Forbidden(message),
            StatusCode::BAD_REQUEST => HttpError::BadRequest(message),
            StatusCode::INTERNAL_SERVER_ERROR => HttpError::Internal(message),
            _ => HttpError::Custom {
                status,
                body: serde_json::json!({ "error": message.as_ref() }),
            },
        }
    }

    /// Shortcut for `HttpError::Internal` with the given message.
    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        HttpError::Internal(message.into())
    }

    /// Shortcut for `HttpError::NotFound` with the given message.
    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        HttpError::NotFound(message.into())
    }

    /// Shortcut for `HttpError::BadRequest` with the given message.
    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        HttpError::BadRequest(message.into())
    }

    /// Shortcut for `HttpError::Unauthorized` with the given message.
    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        HttpError::Unauthorized(message.into())
    }

    /// Shortcut for `HttpError::Forbidden` with the given message.
    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        HttpError::Forbidden(message.into())
    }

    /// Returns the HTTP status code for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            HttpError::NotFound(_) => StatusCode::NOT_FOUND,
            HttpError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            HttpError::Forbidden(_) => StatusCode::FORBIDDEN,
            HttpError::BadRequest(_) | HttpError::Validation(_) => StatusCode::BAD_REQUEST,
            HttpError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            HttpError::Custom { status, .. } | HttpError::WithSource { status, .. } => *status,
        }
    }

    /// Returns the error message, if applicable.
    pub fn message(&self) -> Option<&str> {
        match self {
            HttpError::NotFound(msg)
            | HttpError::Unauthorized(msg)
            | HttpError::Forbidden(msg)
            | HttpError::BadRequest(msg)
            | HttpError::Internal(msg) => Some(msg),
            HttpError::WithSource { message, .. } => Some(message),
            HttpError::Validation(_) => Some("Validation failed"),
            HttpError::Custom { .. } => None,
        }
    }

    /// Adds context to the error message by prefixing it.
    ///
    /// # Example
    /// ```ignore
    /// let err = HttpError::Internal("connection refused".into());
    /// let err = err.context("inserting user");
    /// // message is now: "inserting user: connection refused"
    /// ```
    pub fn context(self, ctx: impl std::fmt::Display) -> Self {
        match self {
            HttpError::NotFound(msg) => HttpError::NotFound(format!("{ctx}: {msg}").into()),
            HttpError::Unauthorized(msg) => HttpError::Unauthorized(format!("{ctx}: {msg}").into()),
            HttpError::Forbidden(msg) => HttpError::Forbidden(format!("{ctx}: {msg}").into()),
            HttpError::BadRequest(msg) => HttpError::BadRequest(format!("{ctx}: {msg}").into()),
            HttpError::Internal(msg) => HttpError::Internal(format!("{ctx}: {msg}").into()),
            HttpError::WithSource { status, message, source } => HttpError::WithSource {
                status,
                message: format!("{ctx}: {message}").into(),
                source,
            },
            other => other, // Validation and Custom are not contextualizable
        }
    }
}

// ── IntoResponse ──────────────────────────────────────────────────────

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
            HttpError::WithSource { status, message, .. } => {
                error_response(status, message)
            }
            other => {
                let (status, message) = match other {
                    HttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
                    HttpError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    HttpError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    HttpError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
                    HttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                    _ => unreachable!(),
                };
                error_response(status, message)
            }
        }
    }
}

// ── Display ───────────────────────────────────────────────────────────

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
            HttpError::WithSource { status, message, .. } => write!(f, "Error ({status}): {message}"),
        }
    }
}

// ── Debug ─────────────────────────────────────────────────────────────

impl std::fmt::Debug for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Display>::fmt(self, f)
    }
}

// ── std::error::Error ─────────────────────────────────────────────────

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HttpError::WithSource { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

// ── From impls ────────────────────────────────────────────────────────

impl From<std::io::Error> for HttpError {
    fn from(err: std::io::Error) -> Self {
        HttpError::Internal(err.to_string().into())
    }
}

// ── HttpErrorExt ──────────────────────────────────────────────────────

/// Extension trait for `Result<T, E>` where `E: Into<HttpError>`.
///
/// Provides a `.http_context()` method to add context to errors ergonomically.
///
/// # Example
/// ```ignore
/// use r2e_core::error::HttpErrorExt;
///
/// let result = db.insert(&user).await
///     .http_context("inserting user")?;
/// ```
pub trait HttpErrorExt<T> {
    /// Convert the error to `HttpError` and prefix context to the message.
    fn http_context(self, ctx: impl std::fmt::Display) -> Result<T, HttpError>;
}

impl<T, E: Into<HttpError>> HttpErrorExt<T> for Result<T, E> {
    fn http_context(self, ctx: impl std::fmt::Display) -> Result<T, HttpError> {
        self.map_err(|e| e.into().context(ctx))
    }
}

/// Generate `From<E> for HttpError` implementations that map error types to
/// a specific `HttpError` variant.
///
/// # Forms
///
/// ## Map to HttpError variants
/// ```ignore
/// r2e_core::map_error! {
///     sqlx::Error => Internal,
///     serde_json::Error => BadRequest,
/// }
/// ```
///
/// ## Map to a custom error type
/// ```ignore
/// r2e_core::map_error! {
///     for MyError {
///         sqlx::Error => DbError,
///     }
/// }
/// ```
#[macro_export]
macro_rules! map_error {
    // Form 1: Map to HttpError (original)
    ( $( $err_ty:ty => $variant:ident ),* $(,)? ) => {
        $(
            impl From<$err_ty> for $crate::HttpError {
                fn from(err: $err_ty) -> Self {
                    $crate::HttpError::$variant(err.to_string().into())
                }
            }
        )*
    };
    // Form 2: Map to a custom error type
    ( for $target:ty { $( $err_ty:ty => $variant:ident ),* $(,)? } ) => {
        $(
            impl From<$err_ty> for $target {
                fn from(err: $err_ty) -> Self {
                    <$target>::$variant(err.to_string().into())
                }
            }
        )*
    };
}

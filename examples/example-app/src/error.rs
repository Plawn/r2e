use r2e::prelude::*;

/// Application-level error type using `#[derive(ApiError)]`.
///
/// Demonstrates best practices for custom error types in R2E:
/// - `#[from]` for automatic conversion from library errors
/// - Explicit status codes and messages per variant
/// - `#[error(transparent)]` to delegate to `HttpError`
#[derive(Debug, ApiError)]
pub enum AppError {
    /// Database errors — mapped from `sqlx::Error` via `#[from]`.
    #[error(status = INTERNAL_SERVER_ERROR, message = "Database error")]
    Database(#[from] sqlx::Error),

    /// Resource not found.
    #[error(status = NOT_FOUND, message = "{0}")]
    NotFound(String),

    /// Bad request / invalid input.
    #[error(status = BAD_REQUEST, message = "{0}")]
    BadRequest(String),

    /// Delegate to the framework's `HttpError` for everything else.
    #[error(transparent)]
    Http(#[from] HttpError),
}

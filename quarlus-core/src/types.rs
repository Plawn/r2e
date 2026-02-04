//! Convenience type aliases for common handler return types.
//!
//! These aliases reduce verbosity in controller methods:
//!
//! ```ignore
//! use quarlus_core::prelude::*;
//!
//! // Before
//! async fn list(&self) -> Result<Json<Vec<User>>, AppError> { ... }
//!
//! // After
//! async fn list(&self) -> JsonResult<Vec<User>> { ... }
//! ```

use crate::error::AppError;
use crate::http::{Json, StatusCode};

/// Flexible result alias — any response type with [`AppError`].
///
/// Use this when the response is not `Json<T>`:
///
/// ```ignore
/// #[get("/health")]
/// async fn health(&self) -> ApiResult<StatusCode> {
///     Ok(StatusCode::OK)
/// }
/// ```
pub type ApiResult<T> = Result<T, AppError>;

/// The most common handler return type — `Result<Json<T>, AppError>`.
///
/// ```ignore
/// #[get("/users")]
/// async fn list(&self) -> JsonResult<Vec<User>> {
///     Ok(Json(self.service.list().await))
/// }
/// ```
pub type JsonResult<T> = Result<Json<T>, AppError>;

/// Shorthand for endpoints that return only a status code (e.g. DELETE).
///
/// ```ignore
/// #[delete("/users/{id}")]
/// async fn delete(&self, Path(id): Path<u64>) -> StatusResult {
///     self.service.delete(id).await?;
///     Ok(StatusCode::NO_CONTENT)
/// }
/// ```
pub type StatusResult = Result<StatusCode, AppError>;

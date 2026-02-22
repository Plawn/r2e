//! Generic managed resource lifecycle support.
//!
//! This module provides the `ManagedResource` trait for resources with
//! automatic acquire/release lifecycle management. Any type implementing
//! this trait can be used with the `#[managed]` attribute on handler parameters.
//!
//! # Overview
//!
//! The `ManagedResource` trait is similar to Python context managers or
//! Java try-with-resources. When a handler parameter is marked with `#[managed]`,
//! the macro-generated code will:
//!
//! 1. Call `acquire()` to obtain the resource before the handler executes
//! 2. Pass a mutable reference (`&mut`) to the handler method
//! 3. Call `release(success)` after the handler completes, where:
//!    - `success = true` if the handler returned `Ok` (or a non-Result type)
//!    - `success = false` if the handler returned `Err`
//!
//! # Error Handling
//!
//! The `ManagedResource` trait requires an error type that implements `Into<Response>`.
//! This module provides two error wrappers:
//!
//! - [`ManagedError`] - Wraps the framework's `HttpError`
//! - [`ManagedErr<E>`] - Generic wrapper for any error type implementing `IntoResponse`
//!
//! # Examples
//!
//! ## Using Custom Error Types
//!
//! ```ignore
//! use r2e_core::{ManagedResource, ManagedErr};
//!
//! // Your custom app error
//! #[derive(Debug)]
//! pub enum MyHttpError {
//!     Database(String),
//!     Internal(String),
//! }
//!
//! impl IntoResponse for MyHttpError {
//!     fn into_response(self) -> Response {
//!         // ... convert to HTTP response
//!     }
//! }
//!
//! // Use ManagedErr<MyHttpError> as the error type
//! impl<S: HasPool + Send + Sync> ManagedResource<S> for Tx<'static, Sqlite> {
//!     type Error = ManagedErr<MyHttpError>;
//!
//!     async fn acquire(state: &S) -> Result<Self, Self::Error> {
//!         let tx = state.pool().begin().await
//!             .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
//!         Ok(Tx(tx))
//!     }
//!
//!     async fn release(self, success: bool) -> Result<(), Self::Error> {
//!         if success {
//!             self.0.commit().await
//!                 .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
//!         }
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Request-scoped Audit Context
//!
//! ```ignore
//! use r2e_core::{ManagedResource, ManagedError, HttpError};
//!
//! pub struct AuditContext {
//!     entries: Vec<String>,
//! }
//!
//! impl AuditContext {
//!     pub fn log(&mut self, message: &str) {
//!         self.entries.push(message.to_string());
//!     }
//! }
//!
//! impl<S: Send + Sync> ManagedResource<S> for AuditContext {
//!     type Error = ManagedError;
//!
//!     async fn acquire(_state: &S) -> Result<Self, Self::Error> {
//!         Ok(AuditContext { entries: Vec::new() })
//!     }
//!
//!     async fn release(self, success: bool) -> Result<(), Self::Error> {
//!         if success {
//!             for entry in self.entries {
//!                 tracing::info!(audit = entry);
//!             }
//!         }
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Database Transaction Wrapper
//!
//! ```ignore
//! use r2e_core::{ManagedResource, ManagedError, HttpError};
//! use sqlx::{Database, Pool, Transaction};
//!
//! /// Transaction wrapper for managed lifecycle
//! pub struct Tx<'a, DB: Database>(pub Transaction<'a, DB>);
//!
//! /// Trait for states containing a database pool
//! pub trait HasPool<DB: Database> {
//!     fn pool(&self) -> &Pool<DB>;
//! }
//!
//! impl<S, DB> ManagedResource<S> for Tx<'static, DB>
//! where
//!     DB: Database,
//!     S: HasPool<DB> + Send + Sync,
//! {
//!     type Error = ManagedError;
//!
//!     async fn acquire(state: &S) -> Result<Self, Self::Error> {
//!         let tx = state.pool().begin().await
//!             .map_err(|e| ManagedError(HttpError::Internal(e.to_string())))?;
//!         Ok(Tx(tx))
//!     }
//!
//!     async fn release(self, success: bool) -> Result<(), Self::Error> {
//!         if success {
//!             self.0.commit().await
//!                 .map_err(|e| ManagedError(HttpError::Internal(e.to_string())))?;
//!         }
//!         // On failure, transaction is dropped and rolled back
//!         Ok(())
//!     }
//! }
//! ```

use crate::error::HttpError;
use crate::http::response::{IntoResponse, Response};
use std::future::Future;

/// A resource with managed lifecycle (acquire/release).
///
/// Similar to Python context managers or Java try-with-resources.
/// The macro-generated handler will:
/// 1. Call `acquire()` to obtain the resource
/// 2. Pass a mutable reference to the handler method
/// 3. Call `release()` with success status based on method result
///
/// # Implementing for Custom Resources
///
/// ```ignore
/// use r2e_core::{ManagedResource, ManagedError, HttpError};
///
/// pub struct MyResource {
///     // ... fields
/// }
///
/// impl<S: Send + Sync> ManagedResource<S> for MyResource {
///     type Error = ManagedError;
///
///     async fn acquire(state: &S) -> Result<Self, Self::Error> {
///         // Acquire/initialize the resource
///         Ok(MyResource { /* ... */ })
///     }
///
///     async fn release(self, success: bool) -> Result<(), Self::Error> {
///         if success {
///             // Commit/finalize on success
///         }
///         // Cleanup happens here or on drop
///         Ok(())
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `ManagedResource<{S}>`",
    label = "this type cannot be used with `#[managed]`",
    note = "implement `ManagedResource<S>` with `acquire()` and `release()` methods for your type"
)]
pub trait ManagedResource<S>: Sized {
    /// Error type returned by acquire/release operations.
    /// Must be convertible to an HTTP response.
    type Error: Into<Response>;

    /// Acquires the resource from the application state.
    ///
    /// Called before the handler method executes.
    fn acquire(state: &S) -> impl Future<Output = Result<Self, Self::Error>> + Send;

    /// Releases the resource after the handler method completes.
    ///
    /// - `success: true` — the handler returned `Ok` (or a non-Result type)
    /// - `success: false` — the handler returned `Err`
    ///
    /// Common patterns:
    /// - Transactions: commit on success, rollback on failure
    /// - Connections: return to pool
    /// - Audit contexts: flush logs on success
    /// - Locks: release the lock
    fn release(self, success: bool) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Error wrapper for managed resource operations using `HttpError`.
///
/// This is a convenience wrapper for use with the framework's built-in `HttpError` type.
/// For custom error types, use [`ManagedErr<E>`] instead.
///
/// # Example
///
/// ```ignore
/// use r2e_core::{ManagedResource, ManagedError, HttpError};
///
/// impl<S: Send + Sync> ManagedResource<S> for MyResource {
///     type Error = ManagedError;
///
///     async fn acquire(_state: &S) -> Result<Self, Self::Error> {
///         Err(ManagedError(HttpError::Internal("failed to acquire".into())))
///     }
///
///     async fn release(self, _success: bool) -> Result<(), Self::Error> {
///         Ok(())
///     }
/// }
/// ```
pub struct ManagedError(pub HttpError);

impl From<HttpError> for ManagedError {
    fn from(err: HttpError) -> Self {
        ManagedError(err)
    }
}

impl From<ManagedError> for Response {
    fn from(err: ManagedError) -> Self {
        err.0.into_response()
    }
}

impl std::fmt::Display for ManagedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Debug for ManagedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ManagedError({:?})", self.0)
    }
}

/// Generic error wrapper for managed resource operations.
///
/// This wrapper allows using any error type that implements `IntoResponse`
/// with the `ManagedResource` trait.
///
/// # Example
///
/// ```ignore
/// use r2e_core::{ManagedResource, ManagedErr};
/// use axum::response::IntoResponse;
///
/// // Your custom error type
/// #[derive(Debug)]
/// pub enum MyHttpError {
///     Database(String),
///     NotFound(String),
/// }
///
/// impl IntoResponse for MyHttpError {
///     fn into_response(self) -> Response {
///         // ... convert to HTTP response
///     }
/// }
///
/// // Use with ManagedResource
/// impl<S: Send + Sync> ManagedResource<S> for MyResource {
///     type Error = ManagedErr<MyHttpError>;
///
///     async fn acquire(_state: &S) -> Result<Self, Self::Error> {
///         Err(ManagedErr(MyHttpError::Database("connection failed".into())))
///     }
///
///     async fn release(self, _success: bool) -> Result<(), Self::Error> {
///         Ok(())
///     }
/// }
/// ```
///
/// # Convenience Methods
///
/// You can use `.into()` or `ManagedErr::from()` for ergonomic error conversion:
///
/// ```ignore
/// let tx = state.pool().begin().await
///     .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
///
/// // Or with From impl:
/// let tx = state.pool().begin().await
///     .map_err(|e| MyHttpError::Database(e.to_string()))?;  // if From<MyHttpError> for ManagedErr<MyHttpError>
/// ```
pub struct ManagedErr<E>(pub E);

impl<E> From<E> for ManagedErr<E> {
    fn from(err: E) -> Self {
        ManagedErr(err)
    }
}

impl<E: IntoResponse> From<ManagedErr<E>> for Response {
    fn from(err: ManagedErr<E>) -> Self {
        err.0.into_response()
    }
}

impl<E: std::fmt::Display> std::fmt::Display for ManagedErr<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<E: std::fmt::Debug> std::fmt::Debug for ManagedErr<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ManagedErr({:?})", self.0)
    }
}

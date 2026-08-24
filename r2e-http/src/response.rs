//! Response types, and the R2E-owned conversion contract R2E code implements.
//!
//! # Two traits, one direction each
//!
//! [`IntoResponse`] is the *backend's* contract (today axum's, re-exported
//! under an R2E name). Handlers must ultimately satisfy it, and composing
//! types — `Result<T, E>`, `(StatusCode, T)`, `Option<T>` — reach it through
//! the backend's own blanket impls, so leaf types have to implement it too.
//!
//! [`IntoHttpResponse`] is R2E's contract. Every R2E type that produces a
//! response (`HttpError`, `ParamError`, `SecurityError`, `TenantError`, …, and
//! everything `#[derive(ApiError)]` generates) implements **this** one, and
//! then bridges to the backend with a single line:
//!
//! ```ignore
//! use r2e_core::http::response::{IntoHttpResponse, Response};
//!
//! impl IntoHttpResponse for MyError {
//!     fn into_http_response(self) -> Response { /* … */ }
//! }
//!
//! r2e_core::http::impl_into_response!(MyError);
//! ```
//!
//! # Why a macro and not a blanket impl
//!
//! The obvious `impl<T: IntoHttpResponse> IntoResponse for T` is an orphan
//! impl — `IntoResponse` is foreign and `T` is not local — so it cannot be
//! written anywhere, including here. The mirror-image blanket
//! (`impl<T: IntoResponse> IntoHttpResponse for T`) *is* legal, but it would
//! conflict with every per-type `IntoHttpResponse` impl and therefore make the
//! migration impossible; it would also leave the backend's trait as the one
//! R2E types implement, which is exactly the coupling this split removes.
//!
//! [`impl_into_response!`] is the resulting bridge: it names the backend
//! contract in **one** place (this file), and a backend swap rewrites that
//! macro instead of every impl site. See
//! `plans/runtime-http-dependency-containment.md` §5.3b.

pub use axum::response::sse::{Event as SseEvent, KeepAlive as SseKeepAlive, Sse};
pub use axum::response::{Html, IntoResponse, Redirect, Response};

/// R2E-owned conversion into an HTTP response.
///
/// The R2E counterpart of the backend's [`IntoResponse`]: R2E crates (and user
/// error types that want to stay backend-neutral) implement this, then emit the
/// backend impl with [`impl_into_response!`].
///
/// Implementations are free to *consume* [`IntoResponse`] while building the
/// value they return — `(StatusCode, Json(body)).into_response()` is the
/// idiomatic body. What 3b removes is R2E code *implementing* the backend's
/// trait, not code calling it.
pub trait IntoHttpResponse {
    /// Consume `self` and produce the HTTP response it stands for.
    fn into_http_response(self) -> Response;
}

/// Bridge one or more [`IntoHttpResponse`] types to the backend's
/// [`IntoResponse`] contract.
///
/// ```ignore
/// r2e_core::http::impl_into_response!(MyError, MyOtherError);
/// ```
///
/// Only non-generic types: a generic type needs the impl written out (that is
/// what `#[derive(ApiError)]` emits).
#[macro_export]
macro_rules! impl_into_response {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl $crate::response::IntoResponse for $ty {
                fn into_response(self) -> $crate::response::Response {
                    <$ty as $crate::response::IntoHttpResponse>::into_http_response(self)
                }
            }
        )+
    };
}

#[doc(inline)]
pub use crate::impl_into_response;


//! State-generic request extraction over HList application states.
//!
//! With the HList state model the application state type is inferred from the
//! builder chain and never written down, so extractors that pull app-scoped
//! beans out of the state (e.g. a JWT validator) cannot implement axum's
//! [`FromRequestParts`] for a nameable state struct anymore. They also cannot
//! write `impl<S, I> FromRequestParts<S> for Me where S: HasBean<Bean, I>` —
//! the index witness `I` would be an unconstrained impl parameter (E0207).
//!
//! This module provides the sanctioned pattern: an R2E-owned extraction trait,
//! [`FromRequestPartsVia`], that carries a **marker slot** `M` in its generics
//! where impls park their witnesses:
//!
//! ```ignore
//! pub struct ViaBean<I>(PhantomData<I>);
//!
//! impl<S, I> FromRequestPartsVia<S, ViaBean<I>> for AuthenticatedUser
//! where
//!     S: HasBean<Arc<JwtClaimsValidator>, I> + Send + Sync,
//! { ... }
//! ```
//!
//! Generated controller code extracts request-scoped values through this trait
//! (via the [`Via`] adapter), threading the markers as inferred generics on the
//! generated `Controller` impl — user code never sees them. Plain axum
//! extractors participate through the blanket [`ViaAxum`] bridge.
//!
//! [`BeanExtract`] is the standalone helper for hand-written axum handlers
//! that need a bean from an HList state: the witness lives in the extractor's
//! own type parameters (`Self`), which is the other E0207-safe position.

use std::marker::PhantomData;

use crate::http::extract::FromRequestParts;
use crate::http::header::Parts;
use crate::type_list::HasBean;

/// Marker for the blanket bridge: any plain axum extractor is also a
/// [`FromRequestPartsVia`] extractor with marker [`ViaAxum`].
pub struct ViaAxum;

/// Marker for bean-backed extractors: the index witness `I` for the
/// underlying [`HasBean`] bound is parked inside the marker.
pub struct ViaBean<I>(PhantomData<fn() -> I>);

/// Marker for optional extraction (`Option<T>` fields/params): wraps the
/// marker of the inner [`OptionalFromRequestPartsVia`] impl.
pub struct ViaOpt<M>(PhantomData<fn() -> M>);

/// State-generic request-parts extraction with a marker slot.
///
/// The R2E counterpart of axum's [`FromRequestParts`]. The extra `M` generic
/// gives impls a place to park index witnesses for `HasBean` bounds (which
/// cannot live on the impl itself — E0207). `M` is always inferred; generated
/// code threads it as an opaque generic.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be extracted from a request against this application state",
    note = "request-scoped types must implement `FromRequestPartsVia` (bean-backed extractors) or axum's `FromRequestParts` for a generic state (bridged automatically)",
    note = "if the extractor pulls a bean from the state, the bean must be registered on the AppBuilder so the `HasBean` bound holds"
)]
pub trait FromRequestPartsVia<S, M>: Sized {
    /// The rejection returned when extraction fails.
    type Rejection: crate::http::response::IntoResponse;

    /// Extract `Self` from request parts and the application state.
    fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send;
}

/// Optional variant of [`FromRequestPartsVia`], mirroring axum's
/// [`OptionalFromRequestParts`]: absence of credentials yields `Ok(None)`
/// instead of a rejection.
pub trait OptionalFromRequestPartsVia<S, M>: Sized {
    /// The rejection returned when extraction fails (not when absent).
    type Rejection: crate::http::response::IntoResponse;

    /// Extract `Option<Self>` from request parts and the application state.
    fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Option<Self>, Self::Rejection>> + Send;
}

// Blanket bridge: every plain axum extractor works, with marker `ViaAxum`.
impl<S, T> FromRequestPartsVia<S, ViaAxum> for T
where
    S: Send + Sync,
    T: FromRequestParts<S>,
{
    type Rejection = T::Rejection;

    fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        T::from_request_parts(parts, state)
    }
}

// NOTE: there is deliberately NO blanket
// `impl<T: OptionalFromRequestParts<S>> OptionalFromRequestPartsVia<S, ViaAxum> for T`.
// `Option<E>` for a plain axum extractor `E` already resolves through the
// `ViaAxum` bridge above (axum provides
// `impl FromRequestParts for Option<T> where T: OptionalFromRequestParts`);
// a second route through `ViaOpt<ViaAxum>` would make the field marker
// ambiguous (E0283) at `register_controller` whenever both apply. Bean-backed
// extractors implement `OptionalFromRequestPartsVia` directly (e.g.
// `AuthenticatedUser` with the `ViaBean<I>` marker) and are reached through
// the `ViaOpt` impl below — they have no axum impl, so no overlap.

// `Option<T>` extracts through the optional trait (marker records the inner
// impl's marker).
impl<S, T, M> FromRequestPartsVia<S, ViaOpt<M>> for Option<T>
where
    S: Send + Sync,
    T: OptionalFromRequestPartsVia<S, M>,
{
    type Rejection = T::Rejection;

    fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        T::from_request_parts_via(parts, state)
    }
}

/// Adapter that turns any [`FromRequestPartsVia`] extractor into a real axum
/// extractor by carrying the marker in its own type (the E0207-safe position).
///
/// Generated handlers declare identity parameters as `Via<T, M>` closure
/// parameters and unwrap `.0` before invoking the route method, so user
/// signatures keep the plain type.
pub struct Via<T, M>(pub T, PhantomData<fn() -> M>);

impl<S, T, M> FromRequestParts<S> for Via<T, M>
where
    S: Send + Sync,
    T: FromRequestPartsVia<S, M>,
{
    type Rejection = T::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        T::from_request_parts_via(parts, state)
            .await
            .map(|value| Via(value, PhantomData))
    }
}

/// Standalone axum extractor that clones a bean of type `T` out of an HList
/// application state.
///
/// For hand-written axum handlers (merged via `merge_router`) that need an
/// app-scoped bean. The index witness `I` lives in the extractor's type
/// parameters and is inferred:
///
/// ```ignore
/// async fn raw_handler(BeanExtract(pool, ..): BeanExtract<SqlitePool>) -> ... { }
/// ```
pub struct BeanExtract<T, I>(pub T, pub PhantomData<fn() -> I>);

impl<S, T, I> FromRequestParts<S> for BeanExtract<T, I>
where
    S: HasBean<T, I> + Send + Sync,
    T: Send,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(BeanExtract(state.get_bean(), PhantomData))
    }
}

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
//!
//! # The bridge-overlap invariant
//!
//! Marker selection is driven by trait inference, so every extractable type
//! must have **exactly one** route to [`FromRequestPartsVia`] for a given
//! state:
//!
//! - plain axum extractors reach it through the blanket [`ViaAxum`] bridge;
//! - bean-backed extractors implement it (and its optional twin) directly,
//!   parking their `HasBean` witness in [`ViaBean`];
//! - `Option<T>` reaches it either through the `ViaAxum` bridge as a whole
//!   (axum provides `FromRequestParts for Option<T>` when
//!   `T: OptionalFromRequestParts`) or through [`ViaOpt`] when
//!   `T: OptionalFromRequestPartsVia` — never both.
//!
//! Consequently a type must NOT implement both axum's `FromRequestParts` /
//! `OptionalFromRequestParts` (generically over the state) and R2E's
//! `FromRequestPartsVia` / `OptionalFromRequestPartsVia`: two applicable
//! routes make the marker ambiguous, and every controller using the type
//! fails at `register_controller()` with an opaque `E0283: type annotations
//! needed` on the inferred marker generics. Rust cannot express the negative
//! bound that would rule this out at impl time, so the invariant is enforced
//! by convention plus an inference probe: extractor authors should pin their
//! type with [`assert_unambiguous_extractor`] in a test (R2E's own
//! bean-backed extractors are pinned this way). Do NOT re-add a blanket
//! `OptionalFromRequestPartsVia<_, ViaAxum>` bridge — it would give every
//! plain axum extractor's `Option<T>` a second route.
//!
//! # The named bridge surface (plan §5.3b)
//!
//! [`FromRequestPartsVia`] is the R2E-owned extraction contract, and every
//! bean-backed extractor R2E ships implements it. A handful of impls of the
//! *backend's* [`FromRequestParts`] necessarily remain, because the backend —
//! not R2E — is what drives extraction of a handler's argument tuple. They are
//! enumerated here so a backend swap has a list instead of a search:
//!
//! | site | why it speaks the backend contract |
//! |---|---|
//! | [`Via<T, M>`](Via) (this file) | *the* reverse bridge: the single adapter from `FromRequestPartsVia` back to the backend. A swap rewrites this impl. |
//! | [`PeerAddr`] (this file) | emitted into the generated handler's argument tuple, and usable as a route-method parameter — both positions the backend extracts. |
//! | [`BeanExtract<T, I>`](BeanExtract) (this file) | exists *for* hand-written backend handlers merged via `merge_router` (see `crate::http::axum_compat`). |
//! | `RequestId` (`crate::builtins::request_id`) | documented route-method parameter. |
//! | `SchedulerHandle` (`r2e-scheduler`) | documented route-method parameter. |
//! | `__R2eRequestData_<C>` (emitted by `#[controller]`) | the generated extractor the backend invokes per request; its *fields* are extracted through `FromRequestPartsVia`. |
//! | `#[derive(Params)]` (emitted by `r2e-macros`) | same, for user parameter structs used as route-method parameters. |
//!
//! Wrapping every route-method parameter in [`Via`] would remove the last five
//! rows, at the cost of an inferred marker generic per parameter (compile time,
//! and `E0283`-shaped errors on user types). That trade was measured and
//! rejected — see the plan. None of these sites names `axum` directly: they all
//! go through `r2e_http`'s re-exported names.

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
    note = "if the extractor pulls a bean from the state, the bean must be registered on the AppBuilder so the `HasBean` bound holds",
    note = "for an `Option<T>` field or parameter, `T` must implement `OptionalFromRequestPartsVia` (bean-backed) or axum's `OptionalFromRequestParts` (bridged automatically)"
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
///
/// Implement this ONLY for bean-backed extractors that have no generic axum
/// `OptionalFromRequestParts` impl — a type with both routes makes the
/// `Option<T>` marker ambiguous (see the module docs on the bridge-overlap
/// invariant, and pin your type with [`assert_unambiguous_extractor`]).
#[diagnostic::on_unimplemented(
    message = "`Option<{Self}>` cannot be extracted from a request against this application state",
    note = "optional extraction requires `{Self}` to implement `OptionalFromRequestPartsVia` (bean-backed extractors) or axum's `OptionalFromRequestParts` for a generic state (bridged automatically)",
    note = "if the extractor pulls a bean from the state, the bean must be registered on the AppBuilder so the `HasBean` bound holds"
)]
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

/// Compile-time probe asserting that `T` has exactly one extraction route
/// (one inferable marker `M`) **against the state `S`**.
///
/// This is the enforcement tool for the bridge-overlap invariant (see the
/// module docs): Rust cannot forbid a type from implementing both axum's
/// extraction traits and R2E's `*Via` traits, but marker inference detects
/// the overlap. Call this in a unit test with the marker left to inference —
/// it compiles iff exactly one route exists against `S`, and fails with
/// `E0283` listing the competing impls if the type has two:
///
/// ```ignore
/// use r2e_core::web::extract::assert_unambiguous_extractor;
/// use r2e_core::type_list::{HCons, HNil};
///
/// type S = HCons<Arc<JwtClaimsValidator>, HNil>;
///
/// assert_unambiguous_extractor::<S, AuthenticatedUser, _>();
/// assert_unambiguous_extractor::<S, Option<AuthenticatedUser>, _>();
/// ```
///
/// **`S` must satisfy every `HasBean` bound the extractor's `*Via` impls
/// carry** (i.e. carry all the beans the extractor reads). The probe only
/// sees the routes *reachable* for `S`: against a state missing the backing
/// bean, the bean-backed route drops out of candidate selection, so a
/// dual-route type would pass the probe on the surviving axum route alone —
/// a silent false pass. A correctly-invariant bean-backed extractor probed
/// with the wrong state fails loudly (E0277 on the missing bean), so build
/// `S` first, then leave the marker to inference.
///
/// Every bean-backed extractor shipped by R2E is pinned this way; authors of
/// custom identity types or `#[inject(request)]` extractors should do the
/// same. Probing `Option<T>` matters even when only `T` is used today — the
/// optional route is the historically fragile one.
pub fn assert_unambiguous_extractor<S, T, M>()
where
    T: FromRequestPartsVia<S, M>,
{
}

/// Infallible extractor for the transport peer address.
///
/// Reads `ConnectInfo<SocketAddr>` from the request extensions, which the
/// server inserts when it is started with connection info (`serve_auto` and the
/// sharded server both are; the QUIC bridge inserts it too). Yields `None`
/// instead of a rejection when it is absent — e.g. under `TestApp`'s in-process
/// dispatch, or behind a service that strips it.
///
/// Generated handlers extract this for guarded routes and pass it to
/// `GuardContext::peer_addr`; hand-written handlers can use it directly.
///
/// Implements the backend's [`FromRequestParts`] rather than
/// [`FromRequestPartsVia`] on purpose — it sits in the generated handler's
/// argument tuple, which the backend extracts. Named bridge point (plan
/// §5.3b); see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub Option<std::net::SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(PeerAddr(
            parts
                .extensions
                .get::<crate::http::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0),
        ))
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
///
/// Implements the backend's [`FromRequestParts`] (not [`FromRequestPartsVia`])
/// by construction: its entire purpose is to be usable from a hand-written
/// backend handler. Named bridge point (plan §5.3b).
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

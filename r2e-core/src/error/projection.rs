//! Where a [`Rejection`] meets its envelope: the per-route projection probe
//! and the app-level [`ErrorProjector`] bean.
//!
//! A route's envelope is read from its **declared return type**: a handler
//! returning `Result<T, E>` with `E: From<Rejection> + IntoHttpResponse +
//! ErrorSchema` projects every failure of the request (extraction, identity,
//! guards, validation, managed resources) through `E`. Any other return type
//! — infallible, or an error type that is not an envelope — falls back to the
//! application projection: the [`ErrorProjector`] bean when one was provided
//! ([`AppBuilder::error_projection`](crate::AppBuilder::error_projection)),
//! else [`HttpError`].
//!
//! The probe is an autoref-specialization pair: generated code calls
//! `(&ProjectionProbe::<ReturnType>::new()).project(rejection, &state)` with
//! both traits imported. The by-value receiver (`ProjectionProbe<Result<T,
//! E>>` with an envelope `E`) wins when it applies; otherwise method lookup
//! autorefs to the `&ProjectionProbe<R>` fallback.

use std::marker::PhantomData;
use std::sync::Arc;

use super::{ErrorSchema, ErrorSchemaInfo, HttpError, Rejection};
use crate::http::response::{IntoHttpResponse, Response};
use crate::type_list::BeanLookup;

/// Application-level error projection, as a bean.
///
/// Built from an envelope type by [`ErrorProjector::of`] and provided through
/// [`AppBuilder::error_projection`](crate::AppBuilder::error_projection). Read
/// on the error path of every route whose return type declares no envelope,
/// and (P4) by the framework-level responders (404/405/413/panic). Absent from
/// the state, [`HttpError`] is used.
///
/// Comparable to a JAX-RS `ExceptionMapper`: one mapping from the typed
/// failure to the wire shape, installed once for the whole application.
#[derive(Clone)]
pub struct ErrorProjector {
    project: Arc<dyn Fn(Rejection) -> Response + Send + Sync>,
    schema: ErrorSchemaInfo,
}

impl ErrorProjector {
    /// Project through the envelope `E`.
    #[must_use]
    pub fn of<E>() -> Self
    where
        E: From<Rejection> + IntoHttpResponse + ErrorSchema + 'static,
    {
        Self {
            project: Arc::new(|rejection| rejection.project::<E>()),
            schema: ErrorSchemaInfo::of::<E>(),
        }
    }

    /// Render a rejection through the installed envelope.
    #[must_use]
    pub fn project(&self, rejection: Rejection) -> Response {
        (self.project)(rejection)
    }

    /// The installed envelope's [`ErrorSchema`], for documentation (the
    /// OpenAPI plugin reads it for every route without a declared envelope).
    #[must_use]
    pub fn schema(&self) -> &ErrorSchemaInfo {
        &self.schema
    }
}

impl std::fmt::Debug for ErrorProjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ErrorProjector({})", self.schema.type_name())
    }
}

/// The application projection: the [`ErrorProjector`] bean when the state
/// holds one, else [`HttpError`].
///
/// Used by generated code for routes without a declared envelope, and by
/// hand-written code that needs the same decision (`TestApp`, framework
/// responders).
#[must_use]
pub fn project_default<S: BeanLookup + ?Sized>(rejection: Rejection, state: &S) -> Response {
    match state.bean_ref::<ErrorProjector>() {
        Some(projector) => projector.project(rejection),
        None => rejection.project::<HttpError>(),
    }
}

/// Autoref probe keyed on a route handler's declared return type `R`.
#[doc(hidden)]
pub struct ProjectionProbe<R: ?Sized>(PhantomData<fn() -> R>);

impl<R: ?Sized> ProjectionProbe<R> {
    #[must_use]
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<R: ?Sized> Default for ProjectionProbe<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// Selected when the return type is `Result<T, E>` with an envelope `E`:
/// project through `E`, ignoring the application projection.
#[doc(hidden)]
pub trait ProjectEnvelope {
    fn project<S: BeanLookup + ?Sized>(&self, rejection: Rejection, state: &S) -> Response;
    /// The envelope's schema, for `RouteInfo::error_schema`.
    fn error_schema(&self) -> Option<ErrorSchemaInfo>;
}

impl<T, E> ProjectEnvelope for ProjectionProbe<Result<T, E>>
where
    E: From<Rejection> + IntoHttpResponse + ErrorSchema,
{
    #[inline]
    fn project<S: BeanLookup + ?Sized>(&self, rejection: Rejection, _state: &S) -> Response {
        rejection.project::<E>()
    }

    #[inline]
    fn error_schema(&self) -> Option<ErrorSchemaInfo> {
        Some(ErrorSchemaInfo::of::<E>())
    }
}

/// Fallback for every other return type: the application projection.
#[doc(hidden)]
pub trait ProjectFallback {
    fn project<S: BeanLookup + ?Sized>(&self, rejection: Rejection, state: &S) -> Response;
    /// No route-level envelope: documented through the application one.
    fn error_schema(&self) -> Option<ErrorSchemaInfo>;
}

impl<R: ?Sized> ProjectFallback for &ProjectionProbe<R> {
    #[inline]
    fn project<S: BeanLookup + ?Sized>(&self, rejection: Rejection, state: &S) -> Response {
        project_default(rejection, state)
    }

    #[inline]
    fn error_schema(&self) -> Option<ErrorSchemaInfo> {
        None
    }
}

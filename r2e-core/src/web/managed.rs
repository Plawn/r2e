//! Cancellation-safe managed resource lifecycle support.
//!
//! A route parameter annotated with `#[managed]` is acquired before the
//! handler runs, exposed to the handler as `&mut R`, and finalized after the
//! handler response has been built. Resources are protected by a
//! [`ManagedGuard`]: if the request is cancelled, panics, or a later resource
//! fails to acquire, [`ManagedResource::abort`] is called from `Drop`.

use crate::http::{
    response::{IntoResponse, Response},
    StatusCode,
};
use crate::web::request_head::RequestHead;
use crate::HttpError;
use std::{future::Future, marker::PhantomData};

/// Static state, application state, and request head made available while
/// acquiring a resource.
#[derive(Debug, Clone, Copy)]
pub struct ManagedContext<'a, S> {
    pub state: &'a S,
    pub controller: &'static str,
    pub handler: &'static str,
    /// The incoming request head, when the resource is acquired on an HTTP
    /// route. `None` for contexts built outside a request (tests, non-HTTP
    /// adapters) — use [`require_request`](Self::require_request) rather than
    /// unwrapping.
    pub request: Option<RequestHead<'a>>,
}

impl<'a, S> ManagedContext<'a, S> {
    #[doc(hidden)]
    pub const fn new(state: &'a S, controller: &'static str, handler: &'static str) -> Self {
        Self {
            state,
            controller,
            handler,
            request: None,
        }
    }

    /// Attach the request head. Called by generated handlers; the head is
    /// extracted once per request and shared by every resource of the route.
    #[doc(hidden)]
    #[must_use]
    pub const fn with_request(mut self, head: RequestHead<'a>) -> Self {
        self.request = Some(head);
        self
    }

    /// The request head, or a uniform 500 when this context was not built from
    /// a request.
    ///
    /// A resource that reads the request (a tenant header, a path parameter, a
    /// correlation id) is only usable on an HTTP route. Every such resource
    /// fails the same way off-request, so the message shape lives here — the
    /// same reasoning as [`missing_bean`](Self::missing_bean).
    pub fn require_request(&self) -> Result<RequestHead<'a>, ManagedErr<HttpError>> {
        self.request.ok_or_else(|| {
            ManagedErr(HttpError::internal(format!(
                "no request context available for {}::{}; this managed resource requires the \
                 request head and can only be acquired on an HTTP route",
                self.controller, self.handler,
            )))
        })
    }

    /// Build the "required bean not found" error for a resource acquired by
    /// type out of the state.
    ///
    /// Every `#[managed]` resource that resolves a bean through
    /// [`BeanLookup`](crate::BeanLookup) can fail the same way — the bean was
    /// never provided — and every one of them wants the same message shape:
    /// what was missing, in which handler, and what to call at build time.
    /// Keeping it here is what makes that message uniform across resources and
    /// across backends.
    #[must_use]
    pub fn missing_bean(&self, prefix: &str, bean: &str, hint: &str) -> ManagedErr<HttpError> {
        ManagedErr(HttpError::internal(format!(
            "{prefix} `{bean}` not found for {}::{}; {hint} before build_state()",
            self.controller, self.handler,
        )))
    }
}

/// Classification of the response produced by a managed handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedOutcomeKind {
    /// Informational, successful, or redirection response.
    Success,
    /// Client or server error response.
    Failure,
}

/// Result of a handler invocation passed to managed resource finalizers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedOutcome {
    pub status: StatusCode,
    pub kind: ManagedOutcomeKind,
}

impl ManagedOutcome {
    pub fn from_status(status: StatusCode) -> Self {
        let kind = if status.is_client_error() || status.is_server_error() {
            ManagedOutcomeKind::Failure
        } else {
            ManagedOutcomeKind::Success
        };
        Self { status, kind }
    }

    pub fn is_success(self) -> bool {
        self.kind == ManagedOutcomeKind::Success
    }
}

/// A request-scoped resource with explicit normal and abort lifecycles.
///
/// `finalize` is awaited on the normal path. `abort` is the synchronous,
/// infallible fallback used when awaiting cleanup is impossible (panic,
/// cancellation, partial acquisition, or a failed finalizer). It must not
/// block and should be idempotent.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `ManagedResource<{S}>`",
    label = "this type cannot be used with `#[managed]`",
    note = "implement `ManagedResource<S>` with `acquire()`, `finalize()`, and `abort()`"
)]
pub trait ManagedResource<S>: Sized + Send {
    /// Error returned while acquiring or finalizing the resource.
    type Error: Into<Response>;

    /// Acquires one resource for the current request.
    fn acquire(
        context: ManagedContext<'_, S>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;

    /// Commits, rolls back, flushes, or otherwise closes the resource.
    fn finalize(
        &mut self,
        outcome: &ManagedOutcome,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Best-effort fallback called by `Drop`; it must be synchronous and
    /// infallible. Async resources should rely on their own drop-safe abort
    /// primitive here (for example SQLx transaction drop rollback).
    fn abort(&mut self);
}

/// Compile-time bean dependencies of a `#[managed]` resource type.
///
/// [`ManagedResource::acquire`] reads its collaborators dynamically
/// ([`BeanLookup`](crate::BeanLookup)), which cannot fail at compile time — a
/// pool that was never provided used to surface as a runtime 500 on the first
/// request. `ManagedDeps` closes that hole: `#[routes]` folds every
/// `#[managed]` parameter type's `Deps` into the controller's dependency list,
/// so a missing bean is a compile error at `register_controller` instead.
///
/// There is deliberately **no blanket impl** — a resource that reads no bean
/// must say so:
///
/// ```ignore
/// impl ManagedDeps for AuditContext {
///     type Deps = TNil;                        // state-only resource
/// }
///
/// impl<DB: Database> ManagedDeps for MyTx<DB> {
///     type Deps = TCons<Pool<DB>, TNil>;       // needs the pool bean
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `ManagedDeps`",
    label = "this `#[managed]` type must declare its bean dependencies",
    note = "implement `ManagedDeps` for `{Self}`: `type Deps = TNil;` when `acquire` reads no bean, \
            or `type Deps = TCons<TheBean, TNil>;` listing every bean it looks up"
)]
pub trait ManagedDeps {
    /// Type-level list ([`TCons`](crate::TCons)/[`TNil`](crate::TNil)) of the
    /// bean types [`ManagedResource::acquire`] looks up.
    type Deps;
}

/// RAII wrapper used by generated handlers.
///
/// This type is public only because route code is generated in the
/// application crate. Applications normally interact with the inner resource
/// through their `#[managed] resource: &mut R` parameter.
#[doc(hidden)]
pub struct ManagedGuard<R, S>
where
    R: ManagedResource<S>,
{
    resource: R,
    armed: bool,
    _state: PhantomData<fn() -> S>,
}

impl<R, S> ManagedGuard<R, S>
where
    R: ManagedResource<S>,
{
    pub async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, R::Error> {
        let resource = R::acquire(context).await?;
        Ok(Self {
            resource,
            armed: true,
            _state: PhantomData,
        })
    }

    pub fn resource_mut(&mut self) -> &mut R {
        &mut self.resource
    }

    pub async fn finalize(&mut self, outcome: &ManagedOutcome) -> Result<(), R::Error> {
        R::finalize(&mut self.resource, outcome).await?;
        self.armed = false;
        Ok(())
    }
}

impl<R, S> Drop for ManagedGuard<R, S>
where
    R: ManagedResource<S>,
{
    fn drop(&mut self) {
        if self.armed {
            self.resource.abort();
        }
    }
}

/// Generic bridge from an `IntoResponse` error to the `Into<Response>` bound
/// required by [`ManagedResource`].
pub struct ManagedErr<E>(pub E);

impl<E> From<E> for ManagedErr<E> {
    fn from(err: E) -> Self {
        Self(err)
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

/// Records a finalization error while allowing remaining resources to close.
#[doc(hidden)]
pub fn record_managed_finalize_error(
    slot: &mut Option<Response>,
    response: Response,
    controller: &'static str,
    handler: &'static str,
) {
    if slot.is_none() {
        *slot = Some(response);
    } else {
        tracing::error!(
            controller,
            handler,
            status = %response.status(),
            "additional managed resource finalization failure"
        );
    }
}


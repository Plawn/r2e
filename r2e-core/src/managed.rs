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
use std::{future::Future, marker::PhantomData};

/// Static and application state made available while acquiring a resource.
#[derive(Debug, Clone, Copy)]
pub struct ManagedContext<'a, S> {
    pub state: &'a S,
    pub controller: &'static str,
    pub handler: &'static str,
}

impl<'a, S> ManagedContext<'a, S> {
    #[doc(hidden)]
    pub const fn new(state: &'a S, controller: &'static str, handler: &'static str) -> Self {
        Self {
            state,
            controller,
            handler,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpError;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct Tracked {
        aborted: Arc<AtomicUsize>,
    }

    impl ManagedResource<Arc<AtomicUsize>> for Tracked {
        type Error = ManagedErr<HttpError>;

        async fn acquire(
            context: ManagedContext<'_, Arc<AtomicUsize>>,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                aborted: context.state.clone(),
            })
        }

        async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
            Ok(())
        }

        fn abort(&mut self) {
            self.aborted.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn outcome_uses_http_status_class() {
        assert!(ManagedOutcome::from_status(StatusCode::CREATED).is_success());
        assert!(ManagedOutcome::from_status(StatusCode::TEMPORARY_REDIRECT).is_success());
        assert!(!ManagedOutcome::from_status(StatusCode::BAD_REQUEST).is_success());
        assert!(!ManagedOutcome::from_status(StatusCode::INTERNAL_SERVER_ERROR).is_success());
    }

    #[tokio::test]
    async fn armed_guard_aborts_on_drop() {
        let aborted = Arc::new(AtomicUsize::new(0));
        let context = ManagedContext::new(&aborted, "Controller", "handler");
        let guard = ManagedGuard::<Tracked, _>::acquire(context).await.unwrap();
        drop(guard);
        assert_eq!(aborted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn finalized_guard_is_disarmed() {
        let aborted = Arc::new(AtomicUsize::new(0));
        let context = ManagedContext::new(&aborted, "Controller", "handler");
        let mut guard = ManagedGuard::<Tracked, _>::acquire(context).await.unwrap();
        guard
            .finalize(&ManagedOutcome::from_status(StatusCode::OK))
            .await
            .unwrap();
        drop(guard);
        assert_eq!(aborted.load(Ordering::SeqCst), 0);
    }
}

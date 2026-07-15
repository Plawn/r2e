use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{
    Guard, GuardContext, HttpError, Interceptor, InterceptorContext, ManagedContext, ManagedErr,
    ManagedOutcome, ManagedResource,
};
use std::future::Future;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

pub struct Allow;

impl SelfBuilt for Allow {}

impl Guard<AuthenticatedUser> for Allow {
    fn check(
        &self,
        _ctx: &GuardContext<'_, AuthenticatedUser>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

pub struct PassThrough;

impl SelfBuilt for PassThrough {}

impl<R: Send> Interceptor<R> for PassThrough {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

pub struct RequestResource;

impl<S: Send + Sync> ManagedResource<S> for RequestResource {
    type Error = ManagedErr<HttpError>;

    async fn acquire(_context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Ok(Self)
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

#[controller(path = "/combined")]
pub struct CombinedController;

#[routes]
#[intercept(PassThrough)]
impl CombinedController {
    #[get("/")]
    #[guard(Allow)]
    async fn combined(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        #[managed] _resource: &mut RequestResource,
    ) -> Result<String, HttpError> {
        Ok(user.sub)
    }
}

fn main() {}

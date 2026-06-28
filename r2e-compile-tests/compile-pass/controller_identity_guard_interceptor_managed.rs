use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{
    Guard, GuardContext, HttpError, Interceptor, InterceptorContext, ManagedErr, ManagedResource,
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

impl Guard<AppState, AuthenticatedUser> for Allow {
    fn check(
        &self,
        _state: &AppState,
        _ctx: &GuardContext<'_, AuthenticatedUser>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

pub struct PassThrough;

impl<R: Send> Interceptor<R, AppState> for PassThrough {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext<'_, AppState>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

pub struct RequestResource;

impl ManagedResource<AppState> for RequestResource {
    type Error = ManagedErr<HttpError>;

    async fn acquire(_state: &AppState) -> Result<Self, Self::Error> {
        Ok(Self)
    }

    async fn release(self, _success: bool) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Controller)]
#[controller(path = "/combined", state = AppState)]
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

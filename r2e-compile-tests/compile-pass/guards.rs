use r2e::prelude::*;
use r2e::{Guard, GuardContext, Identity};
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use std::future::Future;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

pub struct CustomGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for CustomGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[derive(Controller)]
#[controller(path = "/guarded", state = AppState)]
pub struct GuardedController;

#[routes]
impl GuardedController {
    #[get("/admin")]
    #[roles("admin")]
    async fn admin_only(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "admin"
    }

    #[get("/custom")]
    #[guard(CustomGuard)]
    async fn custom_guarded(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "custom"
    }
}

fn main() {}

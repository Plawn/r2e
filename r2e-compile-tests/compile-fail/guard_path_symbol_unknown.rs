use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{Guard, GuardContext, Identity, PathParam};
use serde::Deserialize;
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

#[derive(Clone, Copy, Deserialize)]
pub struct ProjectId(u64);

pub struct ProjectGuard;

impl ProjectGuard {
    pub const fn viewer(_param: PathParam<ProjectId>) -> Self {
        Self
    }
}

impl<S: Send + Sync, I: Identity> Guard<S, I> for ProjectGuard {
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
    #[get("/projects/{id}")]
    #[guard(ProjectGuard::viewer(path::missing))]
    async fn project_guarded(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
        Path(_id): Path<ProjectId>,
    ) -> &'static str {
        "project"
    }
}

fn main() {}

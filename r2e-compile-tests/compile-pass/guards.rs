use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{Guard, GuardContext, GuardError, Identity};
use std::future::Future;
use std::num::ParseIntError;
use std::str::FromStr;
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

#[derive(Clone, Copy)]
pub struct ProjectId(u64);

impl FromStr for ProjectId {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy)]
pub enum ProjectRole {
    Viewer,
}

pub struct ProjectGuard {
    param: &'static str,
    min_role: ProjectRole,
}

impl ProjectGuard {
    pub const fn viewer(param: &'static str) -> Self {
        Self {
            param,
            min_role: ProjectRole::Viewer,
        }
    }
}

impl Guard<AppState, AuthenticatedUser> for ProjectGuard {
    fn check(
        &self,
        _state: &AppState,
        ctx: &GuardContext<'_, AuthenticatedUser>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let _user = ctx
                .identity
                .ok_or_else(|| GuardError::unauthorized("identity required"))?;
            let _project_id: ProjectId = ctx.parse_path_param(self.param)?;
            let _min_role = self.min_role;
            Ok(())
        }
    }
}

#[derive(Controller)]
#[controller(path = "/guarded", state = AppState)]
pub struct GuardedController;

#[routes]
impl GuardedController {
    #[get("/admin")]
    #[roles("admin")]
    async fn admin_only(&self, #[inject(identity)] _user: AuthenticatedUser) -> &'static str {
        "admin"
    }

    #[get("/custom")]
    #[guard(CustomGuard)]
    async fn custom_guarded(&self, #[inject(identity)] _user: AuthenticatedUser) -> &'static str {
        "custom"
    }

    #[get("/projects/{pid}")]
    #[guard(ProjectGuard::viewer("pid"))]
    async fn project_guarded(&self, #[inject(identity)] _user: AuthenticatedUser) -> &'static str {
        "project"
    }
}

fn main() {}

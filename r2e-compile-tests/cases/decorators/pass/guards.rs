use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{Guard, GuardContext, GuardError, Identity, PathParam};
use serde::Deserialize;
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

impl SelfBuilt for CustomGuard {}

impl<I: Identity> Guard<I> for CustomGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[derive(Clone, Copy, Deserialize)]
pub struct ProjectId(u64);

impl FromStr for ProjectId {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy, Deserialize)]
pub struct SbomVersionId(u64);

impl FromStr for SbomVersionId {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy)]
pub enum ProjectRole {
    Viewer,
}

pub trait ProjectParamName {
    fn name(self) -> &'static str;
}

impl ProjectParamName for &'static str {
    fn name(self) -> &'static str {
        self
    }
}

impl ProjectParamName for PathParam<ProjectId> {
    fn name(self) -> &'static str {
        self.name()
    }
}

pub struct ProjectGuard {
    param: &'static str,
    min_role: ProjectRole,
}

impl ProjectGuard {
    pub fn viewer<P: ProjectParamName>(param: P) -> Self {
        Self {
            param: param.name(),
            min_role: ProjectRole::Viewer,
        }
    }
}

impl SelfBuilt for ProjectGuard {}

impl Guard<AuthenticatedUser> for ProjectGuard {
    fn check(
        &self,
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

pub struct SbomGuard {
    project_param: &'static str,
    sbom_param: &'static str,
}

impl SbomGuard {
    pub const fn viewer(
        project_param: PathParam<ProjectId>,
        sbom_param: PathParam<SbomVersionId>,
    ) -> Self {
        Self {
            project_param: project_param.name(),
            sbom_param: sbom_param.name(),
        }
    }
}

impl SelfBuilt for SbomGuard {}

impl Guard<AuthenticatedUser> for SbomGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, AuthenticatedUser>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let _user = ctx
                .identity
                .ok_or_else(|| GuardError::unauthorized("identity required"))?;
            let _project_id: ProjectId = ctx.parse_path_param(self.project_param)?;
            let _sbom_id: SbomVersionId = ctx.parse_path_param(self.sbom_param)?;
            Ok(())
        }
    }
}

#[controller(path = "/guarded")]
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

    #[get("/typed-projects/{id}")]
    #[guard(ProjectGuard::viewer(path::id))]
    async fn typed_project_guarded(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
        Path(_id): Path<ProjectId>,
    ) -> &'static str {
        "typed project"
    }
}

#[controller(path = "/projects/{pid}")]
pub struct SbomController;

#[routes]
impl SbomController {
    #[get("/sboms/{sid}")]
    #[guard(SbomGuard::viewer(path::pid, path::sid))]
    async fn sbom_guarded(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
        Path((_pid, _sid)): Path<(ProjectId, SbomVersionId)>,
    ) -> &'static str {
        "sbom"
    }
}

fn main() {}

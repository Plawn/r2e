use r2e::prelude::*;

use crate::models::{CreateProjectRequest, Project};
use crate::services::ProjectService;
use crate::state::AppState;
use crate::tenant_guard::TenantGuard;
use crate::tenant_identity::TenantUser;

/// Tenant-scoped project endpoints.
/// Uses param-level identity + TenantGuard for tenant isolation.
#[derive(Controller)]
#[controller(path = "/tenants", state = AppState)]
pub struct TenantController {
    #[inject]
    project_service: ProjectService,
}

#[routes]
impl TenantController {
    /// List all projects for a tenant.
    /// TenantGuard ensures the user's tenant_id matches the path.
    #[get("/{tenant_id}/projects")]
    #[guard(TenantGuard)]
    async fn list_projects(
        &self,
        Path(tenant_id): Path<String>,
        #[inject(identity)] _user: TenantUser,
    ) -> Result<Json<Vec<Project>>, AppError> {
        let projects = self.project_service.list_by_tenant(&tenant_id).await?;
        Ok(Json(projects))
    }

    /// Create a project within a tenant.
    /// TenantGuard ensures the user's tenant_id matches the path.
    #[post("/{tenant_id}/projects")]
    #[guard(TenantGuard)]
    async fn create_project(
        &self,
        Path(tenant_id): Path<String>,
        #[inject(identity)] _user: TenantUser,
        Json(body): Json<CreateProjectRequest>,
    ) -> Result<Json<Project>, AppError> {
        let project = self.project_service.create(&tenant_id, body).await?;
        Ok(Json(project))
    }
}

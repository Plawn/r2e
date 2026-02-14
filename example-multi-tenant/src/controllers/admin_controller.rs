use r2e::prelude::*;

use crate::models::TenantInfo;
use crate::services::ProjectService;
use crate::state::AppState;
use crate::tenant_identity::TenantUser;

/// Admin-only endpoints for managing tenants.
#[derive(Controller)]
#[controller(path = "/admin", state = AppState)]
pub struct AdminController {
    #[inject]
    project_service: ProjectService,
}

#[routes]
impl AdminController {
    /// List all tenants with project counts. Requires super-admin role.
    #[get("/tenants")]
    #[roles("super-admin")]
    async fn list_tenants(
        &self,
        #[inject(identity)] _user: TenantUser,
    ) -> Result<Json<Vec<TenantInfo>>, AppError> {
        let tenants = self.project_service.list_tenants().await?;
        Ok(Json(tenants))
    }
}

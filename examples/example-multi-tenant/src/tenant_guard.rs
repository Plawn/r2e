use r2e::prelude::*;
use r2e::{Guard, GuardContext};

use crate::tenant_identity::TenantUser;
use crate::state::AppState;

/// Guard that ensures the authenticated user's tenant_id matches the path parameter.
/// Super-admins bypass this check.
pub struct TenantGuard;

impl Guard<AppState, TenantUser> for TenantGuard {
    fn check(
        &self,
        _state: &AppState,
        ctx: &GuardContext<'_, TenantUser>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        async move {
            let identity = ctx.identity.ok_or_else(|| {
                HttpError::Unauthorized("Authentication required".into()).into_response()
            })?;

            // Super-admins can access any tenant
            if identity.is_super_admin() {
                return Ok(());
            }

            // Extract tenant_id from the path: /tenants/{tenant_id}/...
            let path_tenant = ctx
                .uri
                .path()
                .split('/')
                .nth(2) // ["", "tenants", "{tenant_id}", ...]
                .unwrap_or_default();

            if path_tenant == identity.tenant_id {
                Ok(())
            } else {
                Err(HttpError::Forbidden(format!(
                    "You don't have access to tenant '{}'",
                    path_tenant
                ))
                .into_response())
            }
        }
    }
}

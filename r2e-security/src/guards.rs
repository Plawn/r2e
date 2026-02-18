use r2e_core::guards::{Guard, GuardContext, Identity};
use r2e_core::http::response::{IntoResponse, Response};

/// Extension of [`Identity`] for role-based access control.
///
/// Implement this trait on identity types that carry role information.
/// Used by [`RolesGuard`] to check required roles on handlers annotated with `#[roles("...")]`.
///
/// `AuthenticatedUser` implements this trait automatically.
/// Custom identity types (e.g. `DbUser`, `TenantUser`) must implement it explicitly.
pub trait RoleBasedIdentity: Identity {
    /// Roles associated with this identity.
    fn roles(&self) -> &[String];
}

/// Guard that checks required roles. Returns 403 if missing.
///
/// Applied automatically by `#[roles("admin")]` attribute.
/// Requires the identity type to implement [`RoleBasedIdentity`].
pub struct RolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S: Send + Sync, I: RoleBasedIdentity> Guard<S, I> for RolesGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        let result = (|| {
            let identity = ctx.identity.ok_or_else(|| {
                r2e_core::AppError::Forbidden("No identity available for role check".into())
                    .into_response()
            })?;
            let roles = identity.roles();
            let has_role = self
                .required_roles
                .iter()
                .any(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_role {
                Ok(())
            } else {
                Err(r2e_core::AppError::Forbidden("Insufficient roles".into()).into_response())
            }
        })();
        std::future::ready(result)
    }
}

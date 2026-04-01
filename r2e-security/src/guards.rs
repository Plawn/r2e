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

/// Guard that checks whether the identity has **at least one** of the required roles (OR semantics).
///
/// Returns 403 Forbidden if the identity has none of the required roles.
/// Applied automatically by `#[roles("admin", "editor")]` attribute.
/// Requires the identity type to implement [`RoleBasedIdentity`].
///
/// # Semantics
///
/// `#[roles("admin", "editor")]` passes if the user has `admin` **or** `editor` (or both).
/// For AND semantics (require **all** listed roles), use [`AllRolesGuard`] via `#[all_roles(...)]`.
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
                r2e_core::HttpError::Forbidden("No identity available for role check".into())
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
                Err(r2e_core::HttpError::Forbidden("Insufficient roles".into()).into_response())
            }
        })();
        std::future::ready(result)
    }
}

/// Guard that checks whether the identity has **all** of the required roles (AND semantics).
///
/// Returns 403 Forbidden if the identity is missing any of the required roles.
/// Applied automatically by `#[all_roles("admin", "superadmin")]` attribute.
/// Requires the identity type to implement [`RoleBasedIdentity`].
///
/// # Semantics
///
/// `#[all_roles("admin", "superadmin")]` passes only if the user has **both** `admin` **and** `superadmin`.
/// For OR semantics (require **at least one**), use [`RolesGuard`] via `#[roles(...)]`.
pub struct AllRolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S: Send + Sync, I: RoleBasedIdentity> Guard<S, I> for AllRolesGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        let result = (|| {
            let identity = ctx.identity.ok_or_else(|| {
                r2e_core::HttpError::Forbidden("No identity available for role check".into())
                    .into_response()
            })?;
            let roles = identity.roles();
            let has_all = self
                .required_roles
                .iter()
                .all(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_all {
                Ok(())
            } else {
                Err(r2e_core::HttpError::Forbidden("Insufficient roles".into()).into_response())
            }
        })();
        std::future::ready(result)
    }
}

use r2e_core::decorators::guards::{Guard, GuardContext, Identity};
use r2e_core::http::response::{IntoHttpResponse, Response};
use r2e_core::{Rejection, RejectionKind};

/// Why a role check refused the request.
///
/// Typed so a custom guard (or, later, the projection layer) can match on
/// the cause; converts into a `Forbidden` [`Rejection`] with `?`/`.into()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolesDenied {
    /// The route has no identity to read roles from (an `Option<I>` identity
    /// that came back `None`).
    NoIdentity,
    /// The identity has none of (`#[roles]`) / not all of (`#[all_roles]`)
    /// the required roles.
    Insufficient,
}

impl RolesDenied {
    /// Client-facing message.
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoIdentity => "No identity available for role check",
            Self::Insufficient => "Insufficient roles",
        }
    }
}

impl std::fmt::Display for RolesDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for RolesDenied {}

impl From<RolesDenied> for Rejection {
    fn from(denied: RolesDenied) -> Self {
        Rejection::new(RejectionKind::Forbidden, denied.message())
    }
}

impl IntoHttpResponse for RolesDenied {
    fn into_http_response(self) -> Response {
        Rejection::from(self).into_http_response()
    }
}

r2e_core::http::impl_into_response!(RolesDenied);

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

impl r2e_core::SelfBuilt for RolesGuard {}

impl<I: RoleBasedIdentity> Guard<I> for RolesGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Rejection>> + Send {
        let result = (|| {
            let identity = ctx
                .identity
                .ok_or_else(|| Rejection::from(RolesDenied::NoIdentity))?;
            let roles = identity.roles();
            let has_role = !self.required_roles.is_empty()
                && self
                    .required_roles
                    .iter()
                    .any(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_role {
                Ok(())
            } else {
                Err(Rejection::from(RolesDenied::Insufficient))
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

impl r2e_core::SelfBuilt for AllRolesGuard {}

impl<I: RoleBasedIdentity> Guard<I> for AllRolesGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Rejection>> + Send {
        let result = (|| {
            let identity = ctx
                .identity
                .ok_or_else(|| Rejection::from(RolesDenied::NoIdentity))?;
            let roles = identity.roles();
            let has_all = !self.required_roles.is_empty()
                && self
                    .required_roles
                    .iter()
                    .all(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_all {
                Ok(())
            } else {
                Err(Rejection::from(RolesDenied::Insufficient))
            }
        })();
        std::future::ready(result)
    }
}

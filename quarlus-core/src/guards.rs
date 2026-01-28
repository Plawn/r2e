use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

/// Trait representing an authenticated identity (user, service account, etc.).
///
/// Implement this trait on your identity type (e.g. `AuthenticatedUser`) to
/// decouple guards from a concrete identity struct.
pub trait Identity: Send + Sync {
    /// Unique subject identifier (e.g. JWT "sub" claim).
    fn sub(&self) -> &str;

    /// Roles associated with this identity.
    fn roles(&self) -> &[String];
}

/// Sentinel type representing the absence of an identity.
///
/// Used as the default `IdentityType` in controllers that have no
/// `#[inject(identity)]` field.
pub struct NoIdentity;

impl Identity for NoIdentity {
    fn sub(&self) -> &str {
        ""
    }
    fn roles(&self) -> &[String] {
        &[]
    }
}

/// Context available to guards before the handler body runs.
///
/// Generic over the identity type `I` so that guards can access the full
/// identity object (not just sub/roles strings).
pub struct GuardContext<'a, I: Identity> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub identity: Option<&'a I>,
}

impl<'a, I: Identity> GuardContext<'a, I> {
    /// Convenience accessor for the identity subject.
    pub fn identity_sub(&self) -> Option<&str> {
        self.identity.map(|i| i.sub())
    }

    /// Convenience accessor for the identity roles.
    pub fn identity_roles(&self) -> Option<&[String]> {
        self.identity.map(|i| i.roles())
    }
}

/// Handler-level guard. Runs before the handler body.
/// Returns `Ok(())` to proceed, `Err(Response)` to short-circuit.
///
/// Guards are the handler-level counterpart of `Interceptor<R>` (which is method-level).
/// Built-in guards: `RolesGuard`, `RateLimitGuard` (in `quarlus-rate-limit`).
/// Users can implement custom guards and apply them with `#[guard(expr)]`.
///
/// Generic over both the application state `S` and the identity type `I`.
pub trait Guard<S, I: Identity>: Send + Sync {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> Result<(), Response>;
}

/// Guard that checks required roles. Returns 403 if missing.
pub struct RolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S, I: Identity> Guard<S, I> for RolesGuard {
    fn check(&self, _state: &S, ctx: &GuardContext<'_, I>) -> Result<(), Response> {
        let identity = ctx.identity.ok_or_else(|| {
            crate::AppError::Forbidden("No identity available for role check".into())
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
            Err(crate::AppError::Forbidden("Insufficient roles".into()).into_response())
        }
    }
}

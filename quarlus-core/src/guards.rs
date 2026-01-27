use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

/// Context available to guards before controller construction.
pub struct GuardContext<'a> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub identity_sub: Option<&'a str>,
    pub identity_roles: Option<&'a [String]>,
}

/// Handler-level guard. Runs before controller construction.
/// Returns `Ok(())` to proceed, `Err(Response)` to short-circuit.
///
/// Guards are the handler-level counterpart of `Interceptor<R>` (which is method-level).
/// Built-in guards: `RolesGuard`, `RateLimitGuard` (in `quarlus-rate-limit`).
/// Users can implement custom guards and apply them with `#[guard(expr)]`.
pub trait Guard<S>: Send + Sync {
    fn check(&self, state: &S, ctx: &GuardContext) -> Result<(), Response>;
}

/// Guard that checks required roles. Returns 403 if missing.
pub struct RolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S> Guard<S> for RolesGuard {
    fn check(&self, _state: &S, ctx: &GuardContext) -> Result<(), Response> {
        let roles = ctx.identity_roles.ok_or_else(|| {
            crate::AppError::Forbidden("No identity available for role check".into())
                .into_response()
        })?;
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

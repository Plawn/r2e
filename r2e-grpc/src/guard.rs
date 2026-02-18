use r2e_core::Identity;
use tonic::metadata::MetadataMap;
use tonic::Status;

/// Context available to gRPC guards before the handler body runs.
///
/// Analogous to [`r2e_core::GuardContext`] for HTTP, but carries
/// gRPC-specific data (service name, method name, metadata).
pub struct GrpcGuardContext<'a, I: Identity> {
    pub service_name: &'static str,
    pub method_name: &'static str,
    pub metadata: &'a MetadataMap,
    pub identity: Option<&'a I>,
}

impl<'a, I: Identity> GrpcGuardContext<'a, I> {
    /// Convenience accessor for the identity subject.
    pub fn identity_sub(&self) -> Option<&str> {
        self.identity.map(|i| i.sub())
    }

    /// Convenience accessor for the identity email.
    pub fn identity_email(&self) -> Option<&str> {
        self.identity.and_then(|i| i.email())
    }

    /// Convenience accessor for the identity raw claims.
    pub fn identity_claims(&self) -> Option<&serde_json::Value> {
        self.identity.and_then(|i| i.claims())
    }
}

/// Guard trait for gRPC service methods.
///
/// Runs before the handler body and can short-circuit with a [`tonic::Status`].
/// Analogous to [`r2e_core::Guard`] for HTTP.
///
/// # Example
///
/// ```ignore
/// struct AdminGuard;
///
/// impl<S: Send + Sync, I: RoleBasedIdentity> GrpcGuard<S, I> for AdminGuard {
///     fn check(
///         &self,
///         state: &S,
///         ctx: &GrpcGuardContext<'_, I>,
///     ) -> impl Future<Output = Result<(), Status>> + Send {
///         async move {
///             let identity = ctx.identity
///                 .ok_or_else(|| Status::unauthenticated("No identity"))?;
///             if identity.roles().iter().any(|r| r == "admin") {
///                 Ok(())
///             } else {
///                 Err(Status::permission_denied("Requires admin role"))
///             }
///         }
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `GrpcGuard<{S}, {I}>`",
    label = "this type cannot be used as a gRPC guard",
    note = "implement `GrpcGuard<S, I>` for your type and apply it with `#[guard(YourGuard)]`"
)]
pub trait GrpcGuard<S, I: Identity>: Send + Sync {
    fn check(
        &self,
        state: &S,
        ctx: &GrpcGuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Status>> + Send;
}

/// Built-in gRPC guard that checks required roles.
///
/// Returns `Status::permission_denied` if the identity lacks the required roles.
/// Applied automatically by `#[roles("admin")]` on gRPC methods.
pub struct GrpcRolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S: Send + Sync, I: Identity + GrpcRoleBasedIdentity> GrpcGuard<S, I> for GrpcRolesGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GrpcGuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Status>> + Send {
        let result = (|| {
            let identity = ctx.identity.ok_or_else(|| {
                Status::unauthenticated("No identity available for role check")
            })?;
            let roles = identity.roles();
            let has_role = self
                .required_roles
                .iter()
                .any(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_role {
                Ok(())
            } else {
                Err(Status::permission_denied("Insufficient roles"))
            }
        })();
        std::future::ready(result)
    }
}

/// Extension of [`Identity`] for role-based gRPC access control.
///
/// This is the gRPC equivalent of `r2e_security::RoleBasedIdentity`.
/// Identity types that carry role information should implement this trait.
pub trait GrpcRoleBasedIdentity: Identity {
    /// Roles associated with this identity.
    fn roles(&self) -> &[String];
}

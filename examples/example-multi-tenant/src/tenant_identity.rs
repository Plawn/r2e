use r2e::http::extract::FromRef;
use r2e::Identity;
use r2e::r2e_security::{impl_claims_identity_extractor, AuthenticatedUser, ClaimsIdentity};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A tenant-aware identity that includes the tenant_id from JWT claims.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TenantUser {
    pub auth: AuthenticatedUser,
    pub tenant_id: String,
}

impl Identity for TenantUser {
    fn sub(&self) -> &str {
        self.auth.sub()
    }
    fn roles(&self) -> &[String] {
        self.auth.roles()
    }
    fn email(&self) -> Option<&str> {
        self.auth.email()
    }
    fn claims(&self) -> Option<&serde_json::Value> {
        self.auth.claims()
    }
}

impl TenantUser {
    pub fn is_super_admin(&self) -> bool {
        self.auth.has_role("super-admin")
    }
}

impl<S> ClaimsIdentity<S> for TenantUser
where
    S: Send + Sync,
    Arc<r2e::r2e_security::JwtClaimsValidator>: FromRef<S>,
{
    async fn from_jwt_claims(
        claims: serde_json::Value,
        _state: &S,
    ) -> Result<Self, r2e::AppError> {
        let tenant_id = claims["tenant_id"]
            .as_str()
            .ok_or_else(|| {
                r2e::AppError::Unauthorized("Missing tenant_id claim in JWT".into())
            })?
            .to_owned();

        let auth = AuthenticatedUser::from_claims(claims);

        Ok(TenantUser { auth, tenant_id })
    }
}

impl_claims_identity_extractor!(TenantUser);

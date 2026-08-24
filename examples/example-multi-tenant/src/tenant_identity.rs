use r2e::r2e_security::{
    impl_claims_identity_extractor, AuthenticatedUser, FromValidatedJwtClaims, RoleBasedIdentity,
};
use r2e::{Identity, StandardClaims};
use serde::Serialize;

/// A tenant-aware identity that includes the tenant_id from JWT claims.
// Not `Deserialize`: a trusted identity must never be constructible from a
// request body. Built only via `FromValidatedJwtClaims::from_jwt_claims` after JWT validation.
#[derive(Clone, Debug, Serialize)]
pub struct TenantUser {
    pub auth: AuthenticatedUser,
    pub tenant_id: String,
}

impl Identity for TenantUser {
    fn sub(&self) -> &str {
        self.auth.sub()
    }
    fn email(&self) -> Option<&str> {
        self.auth.email()
    }
    fn claims(&self) -> Option<&StandardClaims> {
        self.auth.claims()
    }
}

impl RoleBasedIdentity for TenantUser {
    fn roles(&self) -> &[String] {
        self.auth.roles()
    }
}

impl TenantUser {
    pub fn is_super_admin(&self) -> bool {
        self.auth.has_role("super-admin")
    }
}

impl<S> FromValidatedJwtClaims<S> for TenantUser
where
    S: Send + Sync,
{
    async fn from_jwt_claims(claims: StandardClaims, _state: &S) -> Result<Self, r2e::HttpError> {
        // `tenant_id` is an application claim, so it lives in `claims.extra`
        // and is read through `StandardClaims::get`.
        let tenant_id = claims
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| r2e::HttpError::unauthorized("Missing tenant_id claim in JWT"))?
            .to_owned();

        let auth = AuthenticatedUser::from_claims(claims);

        Ok(TenantUser { auth, tenant_id })
    }
}

impl_claims_identity_extractor!(TenantUser);

use serde::{Deserialize, Serialize};

use crate::error::SecurityError;

/// Trait for building an identity from validated JWT claims.
///
/// Implement this trait to customize how JWT claims are mapped to your
/// identity type. The `build` method is async, allowing database lookups
/// or other I/O during identity construction.
///
/// The default implementation ([`DefaultIdentityBuilder`]) produces
/// [`AuthenticatedUser`] synchronously from the claims.
///
/// # Example — sync (pure claims mapping)
///
/// ```ignore
/// struct MyIdentityBuilder;
///
/// impl IdentityBuilder for MyIdentityBuilder {
///     type Identity = MyUser;
///     fn build(&self, claims: serde_json::Value)
///         -> impl Future<Output = Result<MyUser, SecurityError>> + Send
///     {
///         let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
///         let tenant = claims.get("tenant_id").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
///         std::future::ready(Ok(MyUser { sub, tenant_id: tenant }))
///     }
/// }
/// ```
///
/// # Example — async (database lookup)
///
/// ```ignore
/// struct DbIdentityBuilder { pool: SqlitePool }
///
/// impl IdentityBuilder for DbIdentityBuilder {
///     type Identity = DbUser;
///     fn build(&self, claims: serde_json::Value)
///         -> impl Future<Output = Result<DbUser, SecurityError>> + Send
///     {
///         let pool = self.pool.clone();
///         async move {
///             let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or_default();
///             sqlx::query_as("SELECT * FROM users WHERE sub = ?")
///                 .bind(sub)
///                 .fetch_one(&pool)
///                 .await
///                 .map_err(|e| SecurityError::ValidationFailed(e.to_string()))
///         }
///     }
/// }
/// ```
pub trait IdentityBuilder: Send + Sync {
    type Identity: Clone + Send + Sync;
    fn build(
        &self,
        claims: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<Self::Identity, SecurityError>> + Send;
}

/// Default identity builder that produces [`AuthenticatedUser`].
///
/// Uses a [`RoleExtractor`] to extract roles from JWT claims.
pub struct DefaultIdentityBuilder {
    role_extractor: Box<dyn RoleExtractor>,
}

impl DefaultIdentityBuilder {
    /// Create a new builder with the [`DefaultRoleExtractor`].
    pub fn new() -> Self {
        Self {
            role_extractor: Box::new(DefaultRoleExtractor),
        }
    }

    /// Create a new builder with a custom role extractor.
    pub fn with_extractor(role_extractor: Box<dyn RoleExtractor>) -> Self {
        Self { role_extractor }
    }
}

impl Default for DefaultIdentityBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityBuilder for DefaultIdentityBuilder {
    type Identity = AuthenticatedUser;

    fn build(
        &self,
        claims: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<AuthenticatedUser, SecurityError>> + Send {
        let user = build_authenticated_user(claims, &*self.role_extractor);
        std::future::ready(Ok(user))
    }
}

/// Represents an authenticated user extracted from a validated JWT token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// Subject claim ("sub") - unique user identifier.
    pub sub: String,

    /// Email claim ("email"), if present in the token.
    pub email: Option<String>,

    /// Roles extracted from the token claims.
    pub roles: Vec<String>,

    /// Raw claims for advanced access.
    pub claims: serde_json::Value,
}

impl quarlus_core::Identity for AuthenticatedUser {
    fn sub(&self) -> &str {
        &self.sub
    }
    fn roles(&self) -> &[String] {
        &self.roles
    }
}

impl AuthenticatedUser {
    /// Check whether the user has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check whether the user has any of the specified roles.
    pub fn has_any_role(&self, roles: &[&str]) -> bool {
        roles.iter().any(|role| self.has_role(role))
    }
}

/// Trait for extracting roles from JWT claims.
///
/// Different OIDC providers store roles in different claim locations.
/// Implement this trait to customize role extraction for your provider.
pub trait RoleExtractor: Send + Sync {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String>;
}

/// Default role extractor that checks common locations:
/// - `roles` (top-level array)
/// - `realm_access.roles` (Keycloak)
pub struct DefaultRoleExtractor;

impl RoleExtractor for DefaultRoleExtractor {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        // Try top-level "roles" claim
        if let Some(roles) = claims.get("roles").and_then(|v| v.as_array()) {
            let extracted: Vec<String> = roles
                .iter()
                .filter_map(|r| r.as_str().map(String::from))
                .collect();
            if !extracted.is_empty() {
                return extracted;
            }
        }

        // Try Keycloak "realm_access.roles"
        if let Some(roles) = claims
            .get("realm_access")
            .and_then(|v| v.get("roles"))
            .and_then(|v| v.as_array())
        {
            let extracted: Vec<String> = roles
                .iter()
                .filter_map(|r| r.as_str().map(String::from))
                .collect();
            if !extracted.is_empty() {
                return extracted;
            }
        }

        Vec::new()
    }
}

/// Build an `AuthenticatedUser` from validated JWT claims using the given role extractor.
pub fn build_authenticated_user(
    claims: serde_json::Value,
    role_extractor: &dyn RoleExtractor,
) -> AuthenticatedUser {
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(String::from);

    let roles = role_extractor.extract_roles(&claims);

    AuthenticatedUser {
        sub,
        email,
        roles,
        claims,
    }
}

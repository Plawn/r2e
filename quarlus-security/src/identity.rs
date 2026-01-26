use serde::{Deserialize, Serialize};

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

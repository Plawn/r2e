//! Keycloak-specific role extraction.
//!
//! This module provides role extractors tailored for Keycloak's JWT token structure.
//!
//! Keycloak stores roles in two locations:
//! - **Realm roles**: `realm_access.roles` — roles assigned at the realm level
//! - **Client roles**: `resource_access.{client_id}.roles` — roles assigned to specific clients
//!
//! # Example Token Structure
//!
//! ```json
//! {
//!   "sub": "user-uuid",
//!   "realm_access": {
//!     "roles": ["realm-admin", "realm-user"]
//!   },
//!   "resource_access": {
//!     "my-api": {
//!       "roles": ["api-admin", "api-reader"]
//!     },
//!     "another-client": {
//!       "roles": ["client-role"]
//!     }
//!   }
//! }
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use r2e_security::keycloak::RoleExtractor;
//! use r2e_security::DefaultIdentityBuilder;
//!
//! // Extract realm roles + roles from "my-api" client
//! let extractor = RoleExtractor::new()
//!     .with_realm_roles()
//!     .with_client("my-api");
//!
//! let builder = DefaultIdentityBuilder::new(extractor);
//! ```

use crate::openid::{self, extract_string_array};

/// Keycloak role extractor for realm-level roles only (`realm_access.roles`).
///
/// Use this when you only need realm roles from Keycloak.
///
/// # Example
///
/// ```ignore
/// use r2e_security::keycloak::RealmRoleExtractor;
/// use r2e_security::DefaultIdentityBuilder;
///
/// let builder = DefaultIdentityBuilder::new(RealmRoleExtractor);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct RealmRoleExtractor;

impl openid::RoleExtractor for RealmRoleExtractor {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        extract_string_array(claims, &["realm_access", "roles"])
    }
}

/// Keycloak role extractor for a single client's roles (`resource_access.{client_id}.roles`).
///
/// Use this when you need roles assigned to a specific Keycloak client.
/// The client ID is stored inline (up to 32 bytes on stack, heap otherwise).
///
/// # Example
///
/// ```ignore
/// use r2e_security::keycloak::ClientRoleExtractor;
/// use r2e_security::DefaultIdentityBuilder;
///
/// let builder = DefaultIdentityBuilder::new(ClientRoleExtractor::new("my-client-id"));
/// ```
#[derive(Debug)]
pub struct ClientRoleExtractor {
    client_id: String,
}

impl ClientRoleExtractor {
    /// Create a new extractor for the given client ID.
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
        }
    }

    /// Returns the client ID this extractor is configured for.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

impl openid::RoleExtractor for ClientRoleExtractor {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        claims
            .get("resource_access")
            .and_then(|v| v.get(self.client_id.as_str()))
            .and_then(|v| v.get("roles"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Keycloak role extractor that combines realm roles and multiple client roles.
///
/// Extracts roles from:
/// - `realm_access.roles` (if enabled via [`with_realm_roles`](Self::with_realm_roles))
/// - `resource_access.{client_id}.roles` for each configured client
///
/// Roles are deduplicated in the output while preserving order.
///
/// # Example
///
/// ```ignore
/// use r2e_security::keycloak::RoleExtractor;
/// use r2e_security::DefaultIdentityBuilder;
///
/// // Realm roles + roles from "my-api" client
/// let extractor = RoleExtractor::new()
///     .with_realm_roles()
///     .with_client("my-api");
///
/// let builder = DefaultIdentityBuilder::new(extractor);
/// ```
///
/// ```ignore
/// // Only client roles from multiple clients
/// let extractor = RoleExtractor::new()
///     .with_client("frontend-app")
///     .with_client("backend-api");
///
/// let builder = DefaultIdentityBuilder::new(extractor);
/// ```
///
/// ```ignore
/// // Realm + multiple clients at once
/// let extractor = RoleExtractor::new()
///     .with_realm_roles()
///     .with_clients(["api", "admin", "web"]);
///
/// let builder = DefaultIdentityBuilder::new(extractor);
/// ```
#[derive(Debug, Default)]
pub struct RoleExtractor {
    include_realm: bool,
    client_ids: Vec<String>,
}

impl RoleExtractor {
    /// Create a new Keycloak role extractor.
    ///
    /// By default, no roles are extracted. Use [`with_realm_roles`](Self::with_realm_roles)
    /// and/or [`with_client`](Self::with_client) to configure which roles to extract.
    pub fn new() -> Self {
        Self::default()
    }

    /// Include realm-level roles (`realm_access.roles`).
    pub fn with_realm_roles(mut self) -> Self {
        self.include_realm = true;
        self
    }

    /// Include client-specific roles (`resource_access.{client_id}.roles`).
    pub fn with_client(mut self, client_id: impl Into<String>) -> Self {
        self.client_ids.push(client_id.into());
        self
    }

    /// Include roles from multiple clients at once.
    pub fn with_clients(
        mut self,
        client_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.client_ids
            .extend(client_ids.into_iter().map(Into::into));
        self
    }

    /// Returns whether realm roles are included.
    pub fn includes_realm(&self) -> bool {
        self.include_realm
    }

    /// Returns the list of client IDs configured.
    pub fn client_ids(&self) -> impl Iterator<Item = &str> {
        self.client_ids.iter().map(|s| s.as_str())
    }
}

impl openid::RoleExtractor for RoleExtractor {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        let mut roles = Vec::new();

        // Extract realm roles
        if self.include_realm {
            roles.extend(extract_string_array(claims, &["realm_access", "roles"]));
        }

        // Extract client roles
        if let Some(resource_access) = claims.get("resource_access") {
            for client_id in &self.client_ids {
                if let Some(client_roles) = resource_access
                    .get(client_id.as_str())
                    .and_then(|v| v.get("roles"))
                    .and_then(|v| v.as_array())
                {
                    roles.extend(
                        client_roles
                            .iter()
                            .filter_map(|r| r.as_str().map(String::from)),
                    );
                }
            }
        }

        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        roles.retain(|r| seen.insert(r.clone()));

        roles
    }
}

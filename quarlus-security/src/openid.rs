//! OpenID Connect role extraction traits and utilities.
//!
//! This module provides the base abstractions for extracting roles from JWT claims.
//! Provider-specific implementations (Keycloak, Auth0, etc.) build on top of these traits.

/// Trait for extracting roles from JWT claims.
///
/// Different OIDC providers store roles in different claim locations.
/// Implement this trait to customize role extraction for your provider.
///
/// # Example
///
/// ```ignore
/// use quarlus_security::openid::RoleExtractor;
///
/// struct MyProviderRoleExtractor;
///
/// impl RoleExtractor for MyProviderRoleExtractor {
///     fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
///         claims
///             .get("custom_roles_claim")
///             .and_then(|v| v.as_array())
///             .map(|arr| arr.iter().filter_map(|r| r.as_str().map(String::from)).collect())
///             .unwrap_or_default()
///     }
/// }
/// ```
pub trait RoleExtractor: Send + Sync {
    /// Extract roles from the given JWT claims.
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String>;
}

/// Standard OIDC role extractor that reads from the top-level `roles` claim.
///
/// This follows the common pattern where roles are stored as a simple array
/// at the root level of the JWT claims.
///
/// # Example
///
/// ```ignore
/// use quarlus_security::openid::StandardRoleExtractor;
/// use quarlus_security::DefaultIdentityBuilder;
///
/// let builder = DefaultIdentityBuilder::new(StandardRoleExtractor);
/// ```
/// Zero-sized type, implements `Copy`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StandardRoleExtractor;

impl RoleExtractor for StandardRoleExtractor {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        extract_string_array(claims, &["roles"])
    }
}

/// Composite role extractor that tries two extractors in order.
///
/// Returns the first non-empty result. For more than two extractors,
/// nest multiple `Composite` instances.
///
/// # Example
///
/// ```ignore
/// use quarlus_security::openid::{Composite, StandardRoleExtractor};
/// use quarlus_security::keycloak::RealmRoleExtractor;
///
/// // Try standard first, fall back to Keycloak realm
/// let extractor = Composite(StandardRoleExtractor, RealmRoleExtractor);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Composite<A, B>(pub A, pub B);

impl<A: RoleExtractor, B: RoleExtractor> RoleExtractor for Composite<A, B> {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        let roles = self.0.extract_roles(claims);
        if !roles.is_empty() {
            roles
        } else {
            self.1.extract_roles(claims)
        }
    }
}

/// Merge role extractor that combines roles from two extractors.
///
/// Unlike [`Composite`] which returns the first non-empty result,
/// this extractor merges roles from both extractors and deduplicates them.
///
/// # Example
///
/// ```ignore
/// use quarlus_security::openid::{Merge, StandardRoleExtractor};
/// use quarlus_security::keycloak::RealmRoleExtractor;
///
/// // Get roles from both standard claim AND Keycloak realm
/// let extractor = Merge(StandardRoleExtractor, RealmRoleExtractor);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Merge<A, B>(pub A, pub B);

impl<A: RoleExtractor, B: RoleExtractor> RoleExtractor for Merge<A, B> {
    fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
        let mut roles = self.0.extract_roles(claims);
        let other = self.1.extract_roles(claims);

        // Deduplicate while preserving order
        let mut seen: std::collections::HashSet<_> = roles.iter().cloned().collect();
        for role in other {
            if seen.insert(role.clone()) {
                roles.push(role);
            }
        }

        roles
    }
}

/// Extract a string array from a nested JSON path.
///
/// # Arguments
///
/// * `value` - The root JSON value
/// * `path` - A slice of keys representing the path to the array
///
/// # Example
///
/// ```ignore
/// use quarlus_security::openid::extract_string_array;
///
/// let claims = serde_json::json!({
///     "realm_access": {
///         "roles": ["admin", "user"]
///     }
/// });
///
/// let roles = extract_string_array(&claims, &["realm_access", "roles"]);
/// assert_eq!(roles, vec!["admin", "user"]);
/// ```
pub fn extract_string_array(value: &serde_json::Value, path: &[&str]) -> Vec<String> {
    let mut current = value;

    for key in path {
        match current.get(*key) {
            Some(v) => current = v,
            None => return Vec::new(),
        }
    }

    current
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_standard_role_extractor() {
        let claims = json!({
            "sub": "user123",
            "roles": ["admin", "user"]
        });

        let extractor = StandardRoleExtractor;
        let roles = extractor.extract_roles(&claims);

        assert_eq!(roles, vec!["admin", "user"]);
    }

    #[test]
    fn test_standard_role_extractor_empty() {
        let claims = json!({
            "sub": "user123"
        });

        let extractor = StandardRoleExtractor;
        let roles = extractor.extract_roles(&claims);

        assert!(roles.is_empty());
    }

    #[test]
    fn test_composite_extractor_first_match() {
        #[derive(Clone, Copy)]
        struct FirstExtractor;
        impl RoleExtractor for FirstExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec!["first".to_string()]
            }
        }

        #[derive(Clone, Copy)]
        struct SecondExtractor;
        impl RoleExtractor for SecondExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec!["second".to_string()]
            }
        }

        let extractor = Composite(FirstExtractor, SecondExtractor);
        let roles = extractor.extract_roles(&json!({}));
        assert_eq!(roles, vec!["first"]);
    }

    #[test]
    fn test_composite_extractor_fallback() {
        #[derive(Clone, Copy)]
        struct EmptyExtractor;
        impl RoleExtractor for EmptyExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec![]
            }
        }

        #[derive(Clone, Copy)]
        struct FallbackExtractor;
        impl RoleExtractor for FallbackExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec!["fallback".to_string()]
            }
        }

        let extractor = Composite(EmptyExtractor, FallbackExtractor);
        let roles = extractor.extract_roles(&json!({}));
        assert_eq!(roles, vec!["fallback"]);
    }

    #[test]
    fn test_merge_extractor() {
        #[derive(Clone, Copy)]
        struct FirstExtractor;
        impl RoleExtractor for FirstExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec!["admin".to_string(), "user".to_string()]
            }
        }

        #[derive(Clone, Copy)]
        struct SecondExtractor;
        impl RoleExtractor for SecondExtractor {
            fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
                vec!["user".to_string(), "guest".to_string()]
            }
        }

        let extractor = Merge(FirstExtractor, SecondExtractor);
        let roles = extractor.extract_roles(&json!({}));
        // "user" should be deduplicated
        assert_eq!(roles, vec!["admin", "user", "guest"]);
    }

    #[test]
    fn test_extract_string_array_nested() {
        let value = json!({
            "a": {
                "b": {
                    "c": ["x", "y", "z"]
                }
            }
        });

        let result = extract_string_array(&value, &["a", "b", "c"]);
        assert_eq!(result, vec!["x", "y", "z"]);
    }

    #[test]
    fn test_extract_string_array_missing_path() {
        let value = json!({
            "a": {
                "b": {}
            }
        });

        let result = extract_string_array(&value, &["a", "b", "c"]);
        assert!(result.is_empty());
    }
}

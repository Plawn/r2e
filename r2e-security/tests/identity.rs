use r2e_core::{Identity, StandardClaims};
use r2e_security::identity::AuthenticatedUser;
use r2e_security::openid::RoleExtractor;
use r2e_security::RoleBasedIdentity;
use serde_json::json;

/// Deserialize a JWT payload into the typed claim set, as the validator does.
fn claims(value: serde_json::Value) -> StandardClaims {
    serde_json::from_value(value).expect("payload deserializes into StandardClaims")
}

// ── Construction from Claims ──

#[test]
fn from_claims_complete() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "user-42",
        "email": "alice@example.com",
        "roles": ["admin", "user"]
    })));
    assert_eq!(user.sub, "user-42");
    assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    assert_eq!(user.roles, vec!["admin", "user"]);
}

#[test]
fn from_claims_missing_sub() {
    let user = AuthenticatedUser::from_claims(claims(json!({ "email": "bob@example.com" })));
    // sub defaults to empty string when missing
    assert_eq!(user.sub, "");
}

#[test]
fn from_claims_missing_email() {
    let user =
        AuthenticatedUser::from_claims(claims(json!({ "sub": "user-1", "roles": ["admin"] })));
    assert!(user.email.is_none());
}

#[test]
fn from_claims_empty_roles() {
    let user = AuthenticatedUser::from_claims(claims(json!({ "sub": "user-1" })));
    assert!(user.roles.is_empty());
}

#[test]
fn from_claims_with_custom_extractor() {
    struct CustomExtractor;
    impl RoleExtractor for CustomExtractor {
        fn extract_roles(&self, claims: &StandardClaims) -> Vec<String> {
            // `custom_roles` is not a standard claim, so it lives in `extra`.
            claims
                .get("custom_roles")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    let user = AuthenticatedUser::from_claims_with(
        claims(json!({
            "sub": "user-1",
            "custom_roles": ["superadmin"],
            "roles": ["should-be-ignored"]
        })),
        &CustomExtractor,
    );
    assert_eq!(user.roles, vec!["superadmin"]);
}

// ── Merge: standard + Keycloak roles ──

#[test]
fn from_claims_merges_standard_and_keycloak_roles() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "user-merge",
        "roles": ["standard-admin"],
        "realm_access": {
            "roles": ["realm-user"]
        }
    })));
    assert!(user.roles.contains(&"standard-admin".to_string()));
    assert!(user.roles.contains(&"realm-user".to_string()));
    assert_eq!(user.roles.len(), 2);
}

#[test]
fn from_claims_deduplicates_merged_roles() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "user-dedup",
        "roles": ["admin", "user"],
        "realm_access": {
            "roles": ["admin", "realm-only"]
        }
    })));
    assert_eq!(user.roles, vec!["admin", "user", "realm-only"]);
}

// ── Role Checking Methods ──

#[test]
fn has_role_present() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["admin", "user"]
    })));
    assert!(user.has_role("admin"));
}

#[test]
fn has_role_absent() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["user"]
    })));
    assert!(!user.has_role("superadmin"));
}

#[test]
fn has_role_case_sensitive() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["admin"]
    })));
    assert!(!user.has_role("Admin"));
}

#[test]
fn has_any_role_one_match() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["admin"]
    })));
    assert!(user.has_any_role(&["admin", "editor"]));
}

#[test]
fn has_any_role_none_match() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["user"]
    })));
    assert!(!user.has_any_role(&["superadmin"]));
}

#[test]
fn has_any_role_empty_slice() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["admin"]
    })));
    assert!(!user.has_any_role(&[]));
}

// ── Identity Trait ──

#[test]
fn identity_sub() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "identity-sub-1", "roles": []
    })));
    assert_eq!(Identity::sub(&user), "identity-sub-1");
}

#[test]
fn identity_roles() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "roles": ["a", "b"]
    })));
    assert_eq!(
        RoleBasedIdentity::roles(&user),
        &["a".to_string(), "b".to_string()]
    );
}

#[test]
fn identity_email() {
    let user = AuthenticatedUser::from_claims(claims(json!({
        "sub": "u", "email": "test@test.com"
    })));
    assert_eq!(Identity::email(&user), Some("test@test.com"));
}

#[test]
fn identity_claims() {
    let user = AuthenticatedUser::from_claims(claims(json!({ "sub": "u", "custom": "value" })));
    let retrieved = Identity::claims(&user).unwrap();
    assert_eq!(retrieved.sub, "u");
    // Non-standard claims stay in `extra`, reachable through `get`.
    assert_eq!(
        retrieved.get("custom").and_then(|v| v.as_str()),
        Some("value")
    );
    // Known claims are fields, deliberately not reachable through `get`.
    assert!(retrieved.get("sub").is_none());
}

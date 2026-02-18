use r2e_security::identity::AuthenticatedUser;
use r2e_security::openid::RoleExtractor;
use r2e_security::RoleBasedIdentity;
use r2e_core::Identity;
use serde_json::json;

// ── Construction from Claims ──

#[test]
fn from_claims_complete() {
    let claims = json!({
        "sub": "user-42",
        "email": "alice@example.com",
        "roles": ["admin", "user"]
    });
    let user = AuthenticatedUser::from_claims(claims);
    assert_eq!(user.sub, "user-42");
    assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    assert_eq!(user.roles, vec!["admin", "user"]);
}

#[test]
fn from_claims_missing_sub() {
    let claims = json!({ "email": "bob@example.com" });
    let user = AuthenticatedUser::from_claims(claims);
    // sub defaults to empty string when missing
    assert_eq!(user.sub, "");
}

#[test]
fn from_claims_missing_email() {
    let claims = json!({ "sub": "user-1", "roles": ["admin"] });
    let user = AuthenticatedUser::from_claims(claims);
    assert!(user.email.is_none());
}

#[test]
fn from_claims_empty_roles() {
    let claims = json!({ "sub": "user-1" });
    let user = AuthenticatedUser::from_claims(claims);
    assert!(user.roles.is_empty());
}

#[test]
fn from_claims_with_custom_extractor() {
    struct CustomExtractor;
    impl RoleExtractor for CustomExtractor {
        fn extract_roles(&self, claims: &serde_json::Value) -> Vec<String> {
            claims
                .get("custom_roles")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default()
        }
    }

    let claims = json!({
        "sub": "user-1",
        "custom_roles": ["superadmin"],
        "roles": ["should-be-ignored"]
    });
    let user = AuthenticatedUser::from_claims_with(claims, &CustomExtractor);
    assert_eq!(user.roles, vec!["superadmin"]);
}

// ── Role Checking Methods ──

#[test]
fn has_role_present() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["admin", "user"]
    }));
    assert!(user.has_role("admin"));
}

#[test]
fn has_role_absent() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["user"]
    }));
    assert!(!user.has_role("superadmin"));
}

#[test]
fn has_role_case_sensitive() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["admin"]
    }));
    assert!(!user.has_role("Admin"));
}

#[test]
fn has_any_role_one_match() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["admin"]
    }));
    assert!(user.has_any_role(&["admin", "editor"]));
}

#[test]
fn has_any_role_none_match() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["user"]
    }));
    assert!(!user.has_any_role(&["superadmin"]));
}

#[test]
fn has_any_role_empty_slice() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["admin"]
    }));
    assert!(!user.has_any_role(&[]));
}

// ── Identity Trait ──

#[test]
fn identity_sub() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "identity-sub-1", "roles": []
    }));
    assert_eq!(Identity::sub(&user), "identity-sub-1");
}

#[test]
fn identity_roles() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "roles": ["a", "b"]
    }));
    assert_eq!(RoleBasedIdentity::roles(&user), &["a".to_string(), "b".to_string()]);
}

#[test]
fn identity_email() {
    let user = AuthenticatedUser::from_claims(json!({
        "sub": "u", "email": "test@test.com"
    }));
    assert_eq!(Identity::email(&user), Some("test@test.com"));
}

#[test]
fn identity_claims() {
    let claims = json!({ "sub": "u", "custom": "value" });
    let user = AuthenticatedUser::from_claims(claims.clone());
    let retrieved = Identity::claims(&user).unwrap();
    assert_eq!(retrieved["custom"], "value");
}

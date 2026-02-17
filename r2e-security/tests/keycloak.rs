use r2e_security::keycloak::{ClientRoleExtractor, RealmRoleExtractor, RoleExtractor};
use r2e_security::openid::RoleExtractor as RoleExtractorTrait;
use serde_json::json;

fn keycloak_claims() -> serde_json::Value {
    json!({
        "sub": "user-uuid",
        "realm_access": {
            "roles": ["realm-admin", "realm-user"]
        },
        "resource_access": {
            "my-api": {
                "roles": ["api-admin", "api-reader"]
            },
            "another-client": {
                "roles": ["client-role"]
            }
        }
    })
}

#[test]
fn test_realm_role_extractor() {
    let claims = keycloak_claims();
    let extractor = RealmRoleExtractor;

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["realm-admin", "realm-user"]);
}

#[test]
fn test_realm_role_extractor_missing() {
    let claims = json!({"sub": "user"});
    let extractor = RealmRoleExtractor;

    let roles = extractor.extract_roles(&claims);
    assert!(roles.is_empty());
}

#[test]
fn test_client_role_extractor() {
    let claims = keycloak_claims();
    let extractor = ClientRoleExtractor::new("my-api");

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["api-admin", "api-reader"]);
}

#[test]
fn test_client_role_extractor_missing_client() {
    let claims = keycloak_claims();
    let extractor = ClientRoleExtractor::new("nonexistent");

    let roles = extractor.extract_roles(&claims);
    assert!(roles.is_empty());
}

#[test]
fn test_combined_realm_only() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new().with_realm_roles();

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["realm-admin", "realm-user"]);
}

#[test]
fn test_combined_client_only() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new().with_client("my-api");

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["api-admin", "api-reader"]);
}

#[test]
fn test_combined_realm_and_client() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new()
        .with_realm_roles()
        .with_client("my-api");

    let roles = extractor.extract_roles(&claims);
    assert_eq!(
        roles,
        vec!["realm-admin", "realm-user", "api-admin", "api-reader"]
    );
}

#[test]
fn test_combined_multiple_clients() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new()
        .with_client("my-api")
        .with_client("another-client");

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["api-admin", "api-reader", "client-role"]);
}

#[test]
fn test_combined_with_clients_iterator() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new().with_clients(["my-api", "another-client"]);

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["api-admin", "api-reader", "client-role"]);
}

#[test]
fn test_combined_deduplication() {
    let claims = json!({
        "realm_access": {
            "roles": ["admin", "user"]
        },
        "resource_access": {
            "client-a": {
                "roles": ["user", "special"]  // "user" is duplicate
            }
        }
    });

    let extractor = RoleExtractor::new()
        .with_realm_roles()
        .with_client("client-a");

    let roles = extractor.extract_roles(&claims);
    assert_eq!(roles, vec!["admin", "user", "special"]);
}

#[test]
fn test_combined_empty_config() {
    let claims = keycloak_claims();
    let extractor = RoleExtractor::new();

    let roles = extractor.extract_roles(&claims);
    assert!(roles.is_empty());
}

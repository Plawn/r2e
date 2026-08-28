//! `ScopePolicy`: how scopes and roles come out of real-world token shapes
//! (plain OAuth, Entra `scp`, Auth0 `permissions`, Keycloak realm/client
//! roles) — exercised through the validator, never by poking claims.

use std::sync::Arc;

use r2e_mcp::auth::{McpAuthError, ScopePolicy};
use r2e_mcp::McpTokenValidator;
use serde_json::json;

use crate::fixtures::{pinned, test_jwt};

fn policy_validator(policy: ScopePolicy) -> McpTokenValidator {
    McpTokenValidator::jwt(Arc::new(test_jwt().claims_validator()), policy)
}

async fn scopes_of(validator: &McpTokenValidator, token: &str) -> Vec<String> {
    let principal = validator.validate(token).await.expect("valid token");
    principal.scopes.to_vec()
}

async fn roles_of(validator: &McpTokenValidator, token: &str) -> Vec<String> {
    let principal = validator.validate(token).await.expect("valid token");
    principal.user.roles.clone()
}

#[tokio::test]
async fn default_ladder_reads_the_scope_claim() {
    let token = test_jwt()
        .token_builder("alice")
        .scopes(&["mcp:read", "mcp:write"])
        .build();
    let validator = pinned(&test_jwt());
    assert_eq!(scopes_of(&validator, &token).await, ["mcp:read", "mcp:write"]);
    let principal = validator.validate(&token).await.unwrap();
    assert!(principal.has_scope("mcp:read"));
    assert!(!principal.has_scope("mcp:admin"));
}

#[tokio::test]
async fn default_ladder_falls_back_to_scp_string() {
    // Entra-style: `scp` as a space-separated string.
    let token = test_jwt()
        .token_builder("alice")
        .claim("scp", "mcp:read mcp:write")
        .build();
    assert_eq!(
        scopes_of(&pinned(&test_jwt()), &token).await,
        ["mcp:read", "mcp:write"]
    );
}

#[tokio::test]
async fn default_ladder_accepts_scp_array() {
    // Okta-style: `scp` as a string array.
    let token = test_jwt()
        .token_builder("alice")
        .claim("scp", json!(["mcp:read", "mcp:admin"]))
        .build();
    assert_eq!(
        scopes_of(&pinned(&test_jwt()), &token).await,
        ["mcp:read", "mcp:admin"]
    );
}

#[tokio::test]
async fn configured_scope_claim_is_authoritative() {
    // Auth0 RBAC: `permissions` array; a configured claim also means the
    // default ladder is IGNORED even when `scope` is present.
    let token = test_jwt()
        .token_builder("alice")
        .scopes(&["should:not:appear"])
        .claim("permissions", json!(["read:x", "write:x"]))
        .build();
    let validator = policy_validator(ScopePolicy {
        scope_claim: Some("permissions".into()),
        ..ScopePolicy::default()
    });
    assert_eq!(scopes_of(&validator, &token).await, ["read:x", "write:x"]);
}

#[tokio::test]
async fn default_roles_merge_flat_and_keycloak_realm_roles() {
    let token = test_jwt()
        .token_builder("alice")
        .roles(&["admin", "user"])
        .realm_roles(&["user", "auditor"])
        .build();
    // Merged, unique, order preserved (flat claim first).
    assert_eq!(
        roles_of(&pinned(&test_jwt()), &token).await,
        ["admin", "user", "auditor"]
    );
}

#[tokio::test]
async fn roles_claim_replaces_the_default_sources() {
    let token = test_jwt()
        .token_builder("alice")
        .roles(&["admin"])
        .claim("groups", json!(["team-a", "team-b"]))
        .build();
    let validator = policy_validator(ScopePolicy {
        roles_claim: Some("groups".into()),
        ..ScopePolicy::default()
    });
    assert_eq!(roles_of(&validator, &token).await, ["team-a", "team-b"]);
}

#[tokio::test]
async fn client_roles_for_merges_keycloak_resource_access() {
    let token = test_jwt()
        .token_builder("alice")
        .roles(&["user"])
        .client_roles("mcp-api", &["uploader"])
        .client_roles("other-api", &["ignored"])
        .build();
    let validator = policy_validator(ScopePolicy {
        client_roles_for: Some("mcp-api".into()),
        ..ScopePolicy::default()
    });
    assert_eq!(roles_of(&validator, &token).await, ["user", "uploader"]);
}

#[tokio::test]
async fn disabled_validator_rejects_everything() {
    let err = McpTokenValidator::disabled()
        .validate("anything")
        .await
        .unwrap_err();
    match err {
        McpAuthError::InvalidToken(reason) => {
            assert_eq!(reason, "token validation is not configured")
        }
        other => panic!("expected InvalidToken, got {other:?}"),
    }
}

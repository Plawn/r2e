use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e_grpc::identity::{extract_jwt_claims_from_metadata, GrpcIdentityExtractor};
use r2e_security::config::SecurityConfig;
use r2e_security::jwt::JwtClaimsValidator;
use tonic::metadata::MetadataMap;

const TEST_SECRET: &[u8] = b"r2e-test-secret-do-not-use-in-production";
const TEST_ISSUER: &str = "test-issuer";
const TEST_AUDIENCE: &str = "test-audience";

fn validator() -> JwtClaimsValidator {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithm(Algorithm::HS256);
    JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config)
}

fn token(sub: &str, exp_offset: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "sub": sub,
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "exp": now + exp_offset,
        "roles": ["admin"],
        "tenant_id": "acme",
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

fn metadata(token: &str) -> MetadataMap {
    let mut map = MetadataMap::new();
    map.insert("authorization", format!("Bearer {token}").parse().unwrap());
    map
}

#[r2e_core::test]
async fn jwt_claims_validator_drives_grpc_identity_extraction() {
    let validator = validator();
    let claims = extract_jwt_claims_from_metadata(&metadata(&token("user-1", 3600)), &validator)
        .await
        .unwrap();
    assert_eq!(claims.sub, "user-1");
    assert_eq!(claims.roles.as_deref(), Some(&["admin".to_string()][..]));
    assert_eq!(
        claims.get("tenant_id").and_then(|v| v.as_str()),
        Some("acme")
    );
}

#[r2e_core::test]
async fn arc_bean_works_through_the_blanket_impl() {
    // The shape apps actually hold: the `Arc<JwtClaimsValidator>` bean.
    let validator = Arc::new(validator());
    let claims =
        GrpcIdentityExtractor::extract_claims(&metadata(&token("user-2", 3600)), &validator)
            .await
            .unwrap();
    assert_eq!(claims.sub, "user-2");
}

#[r2e_core::test]
async fn expired_token_is_unauthenticated() {
    let validator = validator();
    let status = extract_jwt_claims_from_metadata(&metadata(&token("user-1", -3600)), &validator)
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(
        status.message().contains("JWT validation failed"),
        "{status}"
    );
}

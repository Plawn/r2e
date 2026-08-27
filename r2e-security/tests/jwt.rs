use r2e_security::config::SecurityConfig;
use r2e_security::error::SecurityError;
use r2e_security::jwt::{JwtClaimSet, JwtClaimsValidator, JwtValidator};

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};

const TEST_SECRET: &[u8] = b"r2e-test-secret-do-not-use-in-production";
const TEST_ISSUER: &str = "test-issuer";
const TEST_AUDIENCE: &str = "test-audience";

#[derive(serde::Deserialize)]
struct TypedClaims {
    sub: String,
    tenant_id: String,
}

impl JwtClaimSet for TypedClaims {
    fn subject(&self) -> Option<&str> {
        Some(&self.sub)
    }
}

fn test_config() -> SecurityConfig {
    SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithm(Algorithm::HS256)
}

fn test_claims_validator() -> JwtClaimsValidator {
    JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), test_config())
}

fn test_validator() -> JwtValidator {
    JwtValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), test_config())
}

fn make_token(sub: &str, roles: &[&str], email: Option<&str>, exp_offset: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let exp = if exp_offset <= 0 {
        0u64
    } else {
        now + exp_offset as u64
    };

    let mut claims = serde_json::json!({
        "sub": sub,
        "roles": roles,
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "exp": exp,
    });
    if let Some(e) = email {
        claims["email"] = serde_json::Value::String(e.to_string());
    }

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

fn valid_token(sub: &str, roles: &[&str]) -> String {
    make_token(sub, roles, None, 3600)
}

fn encode_claims(claims: &serde_json::Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

// ── JwtClaimsValidator ──

#[r2e_core::test]
async fn validate_valid_token() {
    let validator = test_claims_validator();
    let token = valid_token("user-1", &["admin"]);
    let claims = validator.validate(&token).await.unwrap();
    assert_eq!(claims.sub, "user-1");
    assert_eq!(claims.roles.as_deref(), Some(&["admin".to_string()][..]));
}

#[r2e_core::test]
async fn validate_typed_claims() {
    let validator = test_claims_validator();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1",
        "tenant_id": "tenant-a",
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "exp": now + 3600,
    });

    let claims: TypedClaims = validator
        .validate_as(&encode_claims(&claims))
        .await
        .unwrap();

    assert_eq!(claims.sub, "user-1");
    assert_eq!(claims.tenant_id, "tenant-a");
}

#[r2e_core::test]
async fn validate_expired_token() {
    let validator = test_claims_validator();
    let token = make_token("user-1", &["admin"], None, 0);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::TokenExpired),
        "expected TokenExpired, got: {err}"
    );
}

#[r2e_core::test]
async fn validate_token_before_not_before() {
    let validator = test_claims_validator();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1", "roles": ["admin"],
        "iss": TEST_ISSUER, "aud": TEST_AUDIENCE,
        "exp": now + 3600,
        "nbf": now + 3600,
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let result = validator.validate(&token).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        SecurityError::InvalidToken(_)
    ));
}

#[r2e_core::test]
async fn validate_invalid_signature() {
    let validator = test_claims_validator();

    // Token signed with a different secret
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1", "roles": ["admin"],
        "iss": TEST_ISSUER, "aud": TEST_AUDIENCE,
        "exp": now + 3600,
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"different-secret"),
    )
    .unwrap();

    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::InvalidToken(_)),
        "expected InvalidToken, got: {err}"
    );
}

#[r2e_core::test]
async fn validate_disallowed_algorithm() {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE);
    let validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config);
    let token = valid_token("user-1", &["admin"]);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, SecurityError::ValidationFailed(_)));
}

#[r2e_core::test]
async fn validate_empty_allowed_algorithms() {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithms(std::iter::empty::<Algorithm>());
    let validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config);
    let token = valid_token("user-1", &["admin"]);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, SecurityError::ValidationFailed(_)));
}

#[r2e_core::test]
async fn validate_wrong_issuer() {
    let validator = test_claims_validator();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1", "roles": ["admin"],
        "iss": "wrong-issuer", "aud": TEST_AUDIENCE,
        "exp": now + 3600,
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::ValidationFailed(_)),
        "expected ValidationFailed, got: {err}"
    );
}

#[r2e_core::test]
async fn validate_wrong_audience() {
    let validator = test_claims_validator();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1", "roles": ["admin"],
        "iss": TEST_ISSUER, "aud": "wrong-audience",
        "exp": now + 3600,
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::ValidationFailed(_)),
        "expected ValidationFailed, got: {err}"
    );
}

#[r2e_core::test]
async fn validate_missing_issuer() {
    let validator = test_claims_validator();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1",
        "aud": TEST_AUDIENCE,
        "exp": now + 3600,
    });

    let err = validator
        .validate(&encode_claims(&claims))
        .await
        .unwrap_err();
    assert!(matches!(&err, SecurityError::ValidationFailed(_)));
    assert!(err.to_string().contains("iss"));
}

#[r2e_core::test]
async fn validate_missing_audience() {
    let validator = test_claims_validator();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "user-1",
        "iss": TEST_ISSUER,
        "exp": now + 3600,
    });

    let err = validator
        .validate(&encode_claims(&claims))
        .await
        .unwrap_err();
    assert!(matches!(&err, SecurityError::ValidationFailed(_)));
    assert!(err.to_string().contains("aud"));
}

#[r2e_core::test]
async fn validate_missing_sub() {
    let validator = test_claims_validator();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "roles": ["admin"],
        "iss": TEST_ISSUER, "aud": TEST_AUDIENCE,
        "exp": now + 3600,
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap();

    let result = validator.validate(&token).await;
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::ValidationFailed(_)),
        "expected ValidationFailed, got: {err}"
    );
    assert!(
        err.to_string().contains("sub"),
        "error should mention sub: {err}"
    );
}

#[r2e_core::test]
async fn validate_empty_sub() {
    let validator = test_claims_validator();
    // exp_offset>0 keeps it unexpired; sub is explicitly empty.
    let token = make_token("", &["admin"], None, 3600);

    let result = validator.validate(&token).await;
    let err = result.unwrap_err();
    assert!(
        matches!(err, SecurityError::ValidationFailed(_)),
        "expected ValidationFailed, got: {err}"
    );
}

#[r2e_core::test]
async fn validate_malformed_token() {
    let validator = test_claims_validator();
    let result = validator.validate("not.a.jwt").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        SecurityError::InvalidToken(_)
    ));
}

#[r2e_core::test]
async fn validate_empty_token() {
    let validator = test_claims_validator();
    let result = validator.validate("").await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        SecurityError::InvalidToken(_)
    ));
}

// ── JwtValidator with Identity Builder ──

#[r2e_core::test]
async fn validate_returns_authenticated_user() {
    let validator = test_validator();
    let token = make_token("alice", &["admin", "user"], Some("alice@example.com"), 3600);

    let user = validator.validate(&token).await.unwrap();
    assert_eq!(user.sub, "alice");
    assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    assert_eq!(user.roles, vec!["admin", "user"]);
}

#[r2e_core::test]
async fn validate_claims_passthrough() {
    let validator = test_validator();
    let token = valid_token("user-1", &["admin"]);

    let claims = validator.validate_claims(&token).await.unwrap();
    assert_eq!(claims.sub, "user-1");
}

#[test]
fn claims_validator_accessor() {
    let validator = test_validator();
    let cv = validator.claims_validator();
    assert_eq!(cv.config().issuer, TEST_ISSUER);
}

#[test]
fn config_accessor() {
    let validator = test_validator();
    assert_eq!(validator.config().audience(), TEST_AUDIENCE);
}

#[r2e_core::test]
async fn validate_deserializes_keycloak_shaped_token_into_standard_claims() {
    let validator = test_claims_validator();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = encode_claims(&serde_json::json!({
        "sub": "kc-user",
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "exp": now + 3600,
        "iat": now,
        "email": "kc@example.com",
        "preferred_username": "kcuser",
        "scope": "openid profile",
        "realm_access": { "roles": ["realm-admin"] },
        "resource_access": { "my-api": { "roles": ["api-reader"] } },
        "tenant_id": "acme",
    }));

    let claims = validator.validate(&token).await.unwrap();

    assert_eq!(claims.sub, "kc-user");
    assert_eq!(claims.email.as_deref(), Some("kc@example.com"));
    assert_eq!(claims.preferred_username.as_deref(), Some("kcuser"));
    assert_eq!(claims.iat, Some(now));
    assert_eq!(claims.realm_roles(), ["realm-admin"]);
    assert_eq!(claims.client_roles("my-api"), ["api-reader"]);
    assert_eq!(claims.scopes().collect::<Vec<_>>(), ["openid", "profile"]);
    // Unknown claims survive in `extra`.
    assert_eq!(
        claims.get("tenant_id").and_then(|v| v.as_str()),
        Some("acme")
    );
}

#[r2e_core::test]
async fn validate_as_value_is_still_the_dynamic_escape_hatch() {
    let validator = test_claims_validator();
    let token = valid_token("user-1", &["admin"]);

    let claims: serde_json::Value = validator.validate_as(&token).await.unwrap();
    assert_eq!(claims["sub"].as_str().unwrap(), "user-1");
    assert_eq!(claims["roles"][0].as_str().unwrap(), "admin");
}

// ── audiences / skip_audience_validation / leeway ──

fn claims_validator_with(config: SecurityConfig) -> JwtClaimsValidator {
    JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[r2e_core::test]
async fn any_of_multiple_configured_audiences_accepts_the_token() {
    // `aud` validation is a membership test: a token carrying ANY configured
    // audience passes, including when the token's `aud` is itself an array
    // (the Keycloak/Auth0 shape).
    let config = test_config().with_audiences(["other-api", TEST_AUDIENCE]);
    let validator = claims_validator_with(config);

    let single = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "aud": "other-api", "exp": now_secs() + 3600,
    });
    validator.validate(&encode_claims(&single)).await.unwrap();

    let array = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER,
        "aud": ["account", TEST_AUDIENCE],
        "exp": now_secs() + 3600,
    });
    validator.validate(&encode_claims(&array)).await.unwrap();
}

#[r2e_core::test]
async fn with_audiences_replaces_the_constructor_audience() {
    let config = test_config().with_audiences(["only-this"]);
    let validator = claims_validator_with(config);

    // The constructor audience no longer passes once replaced.
    let token = valid_token("u", &[]);
    assert!(validator.validate(&token).await.is_err());
}

#[r2e_core::test]
async fn skip_audience_validation_accepts_a_token_without_aud() {
    let config = test_config().skip_audience_validation();
    let validator = claims_validator_with(config);

    let no_aud = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "exp": now_secs() + 3600,
    });
    validator.validate(&encode_claims(&no_aud)).await.unwrap();

    let wrong_aud = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "aud": "someone-else", "exp": now_secs() + 3600,
    });
    validator
        .validate(&encode_claims(&wrong_aud))
        .await
        .unwrap();
}

#[r2e_core::test]
async fn skip_audience_validation_still_enforces_issuer_and_exp() {
    let config = test_config().skip_audience_validation();
    let validator = claims_validator_with(config);

    let wrong_iss = serde_json::json!({
        "sub": "u", "iss": "evil", "exp": now_secs() + 3600,
    });
    assert!(validator.validate(&encode_claims(&wrong_iss)).await.is_err());

    let expired = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "exp": 0,
    });
    assert!(validator.validate(&encode_claims(&expired)).await.is_err());
}

#[r2e_core::test]
async fn leeway_tolerates_a_slightly_future_nbf() {
    // A fresh token from a slightly-ahead IdP clock (nbf a few seconds in the
    // future) is rejected with zero leeway and accepted with one.
    let claims = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "aud": TEST_AUDIENCE,
        "exp": now_secs() + 3600,
        "nbf": now_secs() + 30,
    });
    let token = encode_claims(&claims);

    let strict = claims_validator_with(test_config());
    assert!(strict.validate(&token).await.is_err());

    let lenient = claims_validator_with(test_config().with_leeway(60));
    lenient.validate(&token).await.unwrap();
}

#[r2e_core::test]
async fn leeway_tolerates_a_slightly_expired_token() {
    let claims = serde_json::json!({
        "sub": "u", "iss": TEST_ISSUER, "aud": TEST_AUDIENCE,
        "exp": now_secs() - 30,
    });
    let token = encode_claims(&claims);

    let strict = claims_validator_with(test_config());
    assert!(matches!(
        strict.validate(&token).await.unwrap_err(),
        SecurityError::TokenExpired
    ));

    let lenient = claims_validator_with(test_config().with_leeway(60));
    lenient.validate(&token).await.unwrap();
}

/// A mixed-family allow-list (e.g. the RS256+ES256+PS256 default of resource
/// servers) must still validate a token whose algorithm is allowed:
/// jsonwebtoken rejects a `Validation` mixing key families once the key is
/// known, so the validator narrows the list to the token's algorithm.
#[r2e_core::test]
async fn mixed_family_allowed_algorithms_still_validate() {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithms([Algorithm::HS256, Algorithm::ES256]);
    let validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config);
    let token = valid_token("user-1", &["admin"]);
    let claims = validator.validate(&token).await.expect("token must validate");
    assert_eq!(claims.sub, "user-1");
}

use r2e_security::config::SecurityConfig;
use r2e_security::error::SecurityError;
use r2e_security::jwt::{JwtClaimsValidator, JwtValidator};

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};

const TEST_SECRET: &[u8] = b"r2e-test-secret-do-not-use-in-production";
const TEST_ISSUER: &str = "test-issuer";
const TEST_AUDIENCE: &str = "test-audience";

fn test_config() -> SecurityConfig {
    SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithm(Algorithm::HS256)
}

fn test_claims_validator() -> JwtClaimsValidator {
    JwtClaimsValidator::new_with_static_key(
        DecodingKey::from_secret(TEST_SECRET),
        test_config(),
    )
}

fn test_validator() -> JwtValidator {
    JwtValidator::new_with_static_key(
        DecodingKey::from_secret(TEST_SECRET),
        test_config(),
    )
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

// ── JwtClaimsValidator ──

#[tokio::test]
async fn validate_valid_token() {
    let validator = test_claims_validator();
    let token = valid_token("user-1", &["admin"]);
    let claims = validator.validate(&token).await.unwrap();
    assert_eq!(claims["sub"].as_str().unwrap(), "user-1");
    let roles = claims["roles"].as_array().unwrap();
    assert_eq!(roles[0].as_str().unwrap(), "admin");
}

#[tokio::test]
async fn validate_expired_token() {
    let validator = test_claims_validator();
    let token = make_token("user-1", &["admin"], None, 0);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, SecurityError::TokenExpired), "expected TokenExpired, got: {err}");
}

#[tokio::test]
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
    assert!(matches!(err, SecurityError::InvalidToken(_)), "expected InvalidToken, got: {err}");
}

#[tokio::test]
async fn validate_disallowed_algorithm() {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE);
    let validator = JwtClaimsValidator::new_with_static_key(
        DecodingKey::from_secret(TEST_SECRET),
        config,
    );
    let token = valid_token("user-1", &["admin"]);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, SecurityError::ValidationFailed(_)));
}

#[tokio::test]
async fn validate_empty_allowed_algorithms() {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithms(std::iter::empty::<Algorithm>());
    let validator = JwtClaimsValidator::new_with_static_key(
        DecodingKey::from_secret(TEST_SECRET),
        config,
    );
    let token = valid_token("user-1", &["admin"]);
    let result = validator.validate(&token).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, SecurityError::ValidationFailed(_)));
}

#[tokio::test]
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
    assert!(matches!(err, SecurityError::ValidationFailed(_)), "expected ValidationFailed, got: {err}");
}

#[tokio::test]
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
    assert!(matches!(err, SecurityError::ValidationFailed(_)), "expected ValidationFailed, got: {err}");
}

#[tokio::test]
async fn validate_malformed_token() {
    let validator = test_claims_validator();
    let result = validator.validate("not.a.jwt").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SecurityError::InvalidToken(_)));
}

#[tokio::test]
async fn validate_empty_token() {
    let validator = test_claims_validator();
    let result = validator.validate("").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SecurityError::InvalidToken(_)));
}

// ── JwtValidator with Identity Builder ──

#[tokio::test]
async fn validate_returns_authenticated_user() {
    let validator = test_validator();
    let token = make_token("alice", &["admin", "user"], Some("alice@example.com"), 3600);

    let user = validator.validate(&token).await.unwrap();
    assert_eq!(user.sub, "alice");
    assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    assert_eq!(user.roles, vec!["admin", "user"]);
}

#[tokio::test]
async fn validate_claims_passthrough() {
    let validator = test_validator();
    let token = valid_token("user-1", &["admin"]);

    let claims = validator.validate_claims(&token).await.unwrap();
    assert_eq!(claims["sub"].as_str().unwrap(), "user-1");
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
    assert_eq!(validator.config().audience, TEST_AUDIENCE);
}

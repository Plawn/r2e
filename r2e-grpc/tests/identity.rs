use r2e_grpc::identity::{extract_bearer_token, extract_jwt_claims_from_metadata, JwtClaimsValidatorLike};
use tonic::metadata::MetadataMap;

/// Mock JWT claims validator for testing.
struct MockValidator {
    result: Result<serde_json::Value, String>,
}

impl MockValidator {
    fn ok(claims: serde_json::Value) -> Self {
        Self { result: Ok(claims) }
    }

    fn err(msg: &str) -> Self {
        Self {
            result: Err(msg.to_string()),
        }
    }
}

impl JwtClaimsValidatorLike for MockValidator {
    fn validate(
        &self,
        _token: &str,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>>
           + Send {
        let result = self.result.clone().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)) });
        std::future::ready(result)
    }
}

fn metadata_with_bearer(token: &str) -> MetadataMap {
    let mut map = MetadataMap::new();
    map.insert(
        "authorization",
        format!("Bearer {token}").parse().unwrap(),
    );
    map
}

#[test]
fn extract_bearer_token_valid() {
    let metadata = metadata_with_bearer("my-jwt-token");
    let token = extract_bearer_token(&metadata).unwrap();
    assert_eq!(token, "my-jwt-token");
}

#[test]
fn extract_bearer_token_lowercase() {
    let mut metadata = MetadataMap::new();
    metadata.insert(
        "authorization",
        "bearer my-jwt-token".parse().unwrap(),
    );
    let token = extract_bearer_token(&metadata).unwrap();
    assert_eq!(token, "my-jwt-token");
}

#[test]
fn extract_bearer_token_missing_header() {
    let metadata = MetadataMap::new();
    let err = extract_bearer_token(&metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert!(err.message().contains("Missing authorization"));
}

#[test]
fn extract_bearer_token_not_bearer_scheme() {
    let mut metadata = MetadataMap::new();
    metadata.insert("authorization", "Basic abc123".parse().unwrap());
    let err = extract_bearer_token(&metadata).unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert!(err.message().contains("Bearer scheme"));
}

#[tokio::test]
async fn extract_jwt_claims_success() {
    let metadata = metadata_with_bearer("valid-token");
    let claims = serde_json::json!({ "sub": "user-1", "roles": ["admin"] });
    let validator = MockValidator::ok(claims.clone());

    let result = extract_jwt_claims_from_metadata(&metadata, &validator).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), claims);
}

#[tokio::test]
async fn extract_jwt_claims_validation_failure() {
    let metadata = metadata_with_bearer("invalid-token");
    let validator = MockValidator::err("token expired");

    let result = extract_jwt_claims_from_metadata(&metadata, &validator).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(status.message().contains("JWT validation failed"));
}

#[tokio::test]
async fn extract_jwt_claims_missing_auth() {
    let metadata = MetadataMap::new();
    let validator = MockValidator::ok(serde_json::json!({}));

    let result = extract_jwt_claims_from_metadata(&metadata, &validator).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn arc_validator_works() {
    use std::sync::Arc;

    let claims = serde_json::json!({ "sub": "user-arc" });
    let validator = Arc::new(MockValidator::ok(claims.clone()));
    let metadata = metadata_with_bearer("arc-token");

    let result = extract_jwt_claims_from_metadata(&metadata, &*validator).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), claims);
}

//! Coverage for `impl_claims_identity_extractor!` against the
//! `FromRequestPartsVia`/`HasBean` extraction model.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e_core::extract::{FromRequestPartsVia, OptionalFromRequestPartsVia, ViaBean};
use r2e_core::http::header::{HttpRequest, Parts, AUTHORIZATION};
use r2e_core::type_list::{HCons, HNil, Here};
use r2e_core::Identity;
use r2e_security::{
    impl_claims_identity_extractor, FromValidatedJwtClaims, JwtClaimSet, JwtClaimsValidator,
    SecurityConfig,
};
use serde::Deserialize;

const TEST_SECRET: &[u8] = b"claims-identity-test-secret";
const TEST_ISSUER: &str = "claims-identity-test";
const TEST_AUDIENCE: &str = "claims-identity-audience";

struct CustomUser {
    sub: String,
}

#[derive(Deserialize)]
struct CustomClaims {
    sub: String,
    #[serde(default)]
    reject: bool,
}

impl JwtClaimSet for CustomClaims {
    fn subject(&self) -> Option<&str> {
        Some(&self.sub)
    }
}

impl Identity for CustomUser {
    fn sub(&self) -> &str {
        &self.sub
    }
}

impl<S: Send + Sync> FromValidatedJwtClaims<S, CustomClaims> for CustomUser {
    async fn from_jwt_claims(
        claims: CustomClaims,
        _state: &S,
    ) -> Result<Self, r2e_core::HttpError> {
        if claims.reject {
            return Err(r2e_core::HttpError::forbidden(
                "identity construction rejected",
            ));
        }

        Ok(CustomUser { sub: claims.sub })
    }
}

// The trailing comma is intentional. CustomUser also deliberately does not
// implement Clone: request extraction only needs an owned identity.
impl_claims_identity_extractor!(CustomUser, claims = CustomClaims,);

type TestState = HCons<Arc<JwtClaimsValidator>, HNil>;
type TestMarker = ViaBean<Here>;

fn test_state() -> TestState {
    let config = SecurityConfig::new("unused", TEST_ISSUER, TEST_AUDIENCE)
        .with_allowed_algorithm(Algorithm::HS256);
    let validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(TEST_SECRET), config);
    HCons {
        head: Arc::new(validator),
        tail: HNil,
    }
}

fn token(reject: bool) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "custom-user",
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
        "exp": now + 3600,
        "reject": reject,
    });

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

fn request_parts(authorization: Option<String>) -> Parts {
    let mut request = HttpRequest::builder().uri("/test");
    if let Some(value) = authorization {
        request = request.header(AUTHORIZATION, value);
    }
    request.body(()).unwrap().into_parts().0
}

#[r2e_core::test]
async fn required_extractor_builds_identity_from_validated_claims() {
    let state = test_state();
    let mut parts = request_parts(Some(format!("Bearer {}", token(false))));

    let user = <CustomUser as FromRequestPartsVia<TestState, TestMarker>>::from_request_parts_via(
        &mut parts, &state,
    )
    .await
    .unwrap();

    assert_eq!(user.sub(), "custom-user");
}

#[r2e_core::test]
async fn optional_extractor_returns_none_without_authorization_header() {
    let state = test_state();
    let mut parts = request_parts(None);

    let user =
        <CustomUser as OptionalFromRequestPartsVia<TestState, TestMarker>>::from_request_parts_via(
            &mut parts, &state,
        )
        .await
        .unwrap();

    assert!(user.is_none());
}

#[r2e_core::test]
async fn optional_extractor_rejects_invalid_token_when_header_is_present() {
    let state = test_state();
    let mut parts = request_parts(Some("Bearer invalid-token".into()));

    let error =
        <CustomUser as OptionalFromRequestPartsVia<TestState, TestMarker>>::from_request_parts_via(
            &mut parts, &state,
        )
        .await
        .err()
        .expect("an invalid token must be rejected");

    assert_eq!(error.status(), r2e_core::http::StatusCode::UNAUTHORIZED);
}

#[r2e_core::test]
async fn identity_construction_error_is_propagated() {
    let state = test_state();
    let mut parts = request_parts(Some(format!("Bearer {}", token(true))));

    let error = <CustomUser as FromRequestPartsVia<TestState, TestMarker>>::from_request_parts_via(
        &mut parts, &state,
    )
    .await
    .err()
    .expect("identity construction errors must be propagated");

    assert_eq!(error.status(), r2e_core::http::StatusCode::FORBIDDEN);
    assert_eq!(error.message(), Some("identity construction rejected"));
}

/// Bridge-overlap invariant pin: the macro-generated `*Via` impls must be
/// `CustomUser`'s only extraction route (see `r2e-core/src/extract.rs`).
#[test]
fn macro_generated_extraction_route_is_unambiguous() {
    use r2e_core::extract::assert_unambiguous_extractor;

    assert_unambiguous_extractor::<TestState, CustomUser, _>();
    assert_unambiguous_extractor::<TestState, Option<CustomUser>, _>();
}

#[allow(deprecated)]
#[test]
fn legacy_claims_identity_name_remains_compatible() {
    fn assert_legacy_impl<T: r2e_security::ClaimsIdentity<TestState, CustomClaims>>() {}

    assert_legacy_impl::<CustomUser>();
}

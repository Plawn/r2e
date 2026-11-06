use r2e_security::error::SecurityError;
use r2e_security::extractor::{extract_bearer_token, extract_bearer_token_from_parts};

#[test]
fn valid_bearer_token() {
    let result = extract_bearer_token("Bearer abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn case_insensitive_scheme() {
    let result = extract_bearer_token("bearer abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn case_insensitive_scheme_mixed() {
    let result = extract_bearer_token("BEARER abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn invalid_scheme_basic() {
    let result = extract_bearer_token("Basic abc123");
    assert!(matches!(result, Err(SecurityError::InvalidAuthScheme)));
}

#[test]
fn empty_authorization_header() {
    let result = extract_bearer_token("");
    assert!(matches!(result, Err(SecurityError::InvalidAuthScheme)));
}

#[test]
fn bearer_only_no_token() {
    // "Bearer " splits into ["Bearer", ""] — returns empty string
    let result = extract_bearer_token("Bearer ");
    assert_eq!(result.unwrap(), "");
}

#[test]
fn extra_whitespace_is_trimmed() {
    // Extra spaces between scheme and token must not leak into the token.
    assert_eq!(extract_bearer_token("Bearer   abc123").unwrap(), "abc123");
}

#[test]
fn token_with_dots() {
    let result = extract_bearer_token("Bearer eyJ.eyJ.sig");
    assert_eq!(result.unwrap(), "eyJ.eyJ.sig");
}

#[test]
fn missing_authorization_header() {
    use r2e_core::http::header::HttpRequest;
    let (parts, _) = HttpRequest::builder()
        .uri("/test")
        .body(())
        .unwrap()
        .into_parts();
    let result = extract_bearer_token_from_parts(&parts);
    assert!(matches!(result, Err(SecurityError::MissingAuthHeader)));
}

/// Bridge-overlap invariant pin (see `r2e-core/src/extract.rs` module docs):
/// `AuthenticatedUser` must have exactly ONE extraction route — the
/// `ViaBean`-marked `FromRequestPartsVia`/`OptionalFromRequestPartsVia`
/// impls. Adding a generic axum `FromRequestParts`/`OptionalFromRequestParts`
/// impl for it would create a second route and turn every controller using
/// it into an opaque `E0283` at `register_controller()` — this probe fails
/// first, with the competing impls listed.
#[test]
fn authenticated_user_extraction_route_is_unambiguous() {
    use r2e_core::type_list::{HCons, HNil};
    use r2e_core::web::extract::assert_unambiguous_extractor;
    use r2e_security::{AuthenticatedUser, JwtClaimsValidator};
    use std::sync::Arc;

    type S = HCons<Arc<JwtClaimsValidator>, HNil>;
    assert_unambiguous_extractor::<S, AuthenticatedUser, _>();
    assert_unambiguous_extractor::<S, Option<AuthenticatedUser>, _>();
}

// ── The rejection carries the RFC 6750 challenge (#1173) ──────────────────
//
// `AuthenticatedUser`'s extractor rejects with `r2e_core::Rejection`, so the
// `WWW-Authenticate` header attached by `From<SecurityError>` reaches the
// wire — through `#[routes(error = E)]` projection too (`Rejection::project`
// keeps hub headers; pinned in `r2e-core/tests/http/projection.rs`).
mod challenge {
    use std::sync::Arc;

    use jsonwebtoken::{Algorithm, DecodingKey};
    use r2e_core::http::header::{HttpRequest, Parts, AUTHORIZATION, WWW_AUTHENTICATE};
    use r2e_core::type_list::{HCons, HNil, Here};
    use r2e_core::web::extract::{FromRequestPartsVia, OptionalFromRequestPartsVia, ViaBean};
    use r2e_core::{Rejection, RejectionKind};
    use r2e_security::{AuthenticatedUser, JwtClaimsValidator, SecurityConfig};

    type State = HCons<Arc<JwtClaimsValidator>, HNil>;
    type Marker = ViaBean<Here>;

    fn state() -> State {
        let config = SecurityConfig::new("unused", "challenge-test", "challenge-audience")
            .with_allowed_algorithm(Algorithm::HS256);
        let validator = JwtClaimsValidator::new_with_static_key(
            DecodingKey::from_secret(b"challenge-test-secret"),
            config,
        );
        HCons {
            head: Arc::new(validator),
            tail: HNil,
        }
    }

    fn parts(authorization: Option<&str>) -> Parts {
        let mut request = HttpRequest::builder().uri("/test");
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
        }
        request.body(()).unwrap().into_parts().0
    }

    async fn required(authorization: Option<&str>) -> Rejection {
        <AuthenticatedUser as FromRequestPartsVia<State, Marker>>::from_request_parts_via(
            &mut parts(authorization),
            &state(),
        )
        .await
        .err()
        .expect("the request must be rejected")
    }

    fn challenge(rejection: &Rejection) -> &str {
        rejection
            .headers()
            .get(WWW_AUTHENTICATE)
            .expect("a 401 carries WWW-Authenticate")
            .to_str()
            .unwrap()
    }

    #[r2e_core::test]
    async fn missing_credentials_get_a_bare_bearer_challenge() {
        let rejection = required(None).await;
        assert_eq!(rejection.kind, RejectionKind::Unauthenticated);
        assert_eq!(challenge(&rejection), "Bearer");
    }

    #[r2e_core::test]
    async fn wrong_scheme_gets_a_bare_bearer_challenge() {
        let rejection = required(Some("Basic dXNlcjpwYXNz")).await;
        assert_eq!(rejection.kind, RejectionKind::Unauthenticated);
        assert_eq!(challenge(&rejection), "Bearer");
    }

    #[r2e_core::test]
    async fn refused_token_gets_an_invalid_token_challenge() {
        let rejection = required(Some("Bearer not-a-jwt")).await;
        assert_eq!(rejection.kind, RejectionKind::Unauthenticated);
        assert_eq!(
            challenge(&rejection),
            r#"Bearer error="invalid_token", error_description="The access token is invalid""#
        );
    }

    #[r2e_core::test]
    async fn optional_identity_rejects_a_refused_token_with_the_challenge() {
        let rejection =
            <AuthenticatedUser as OptionalFromRequestPartsVia<State, Marker>>::from_request_parts_via(
                &mut parts(Some("Bearer not-a-jwt")),
                &state(),
            )
            .await
            .err()
            .expect("a presented but invalid token is rejected");
        assert_eq!(
            challenge(&rejection),
            r#"Bearer error="invalid_token", error_description="The access token is invalid""#
        );
    }

    /// The challenge survives the HTTP rendering of the rejection.
    #[r2e_core::test]
    async fn the_rendered_401_carries_the_challenge() {
        use r2e_core::http::{IntoHttpResponse, StatusCode};
        let response = required(None).await.into_http_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(WWW_AUTHENTICATE).map(|v| v.to_str().unwrap()),
            Some("Bearer")
        );
    }
}

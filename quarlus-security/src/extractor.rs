use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use tracing::{debug, warn};

use crate::error::SecurityError;
use crate::identity::AuthenticatedUser;
use crate::jwt::JwtValidator;

/// Extract a Bearer token from the Authorization header value.
fn extract_bearer_token(header_value: &str) -> Result<&str, SecurityError> {
    let parts: Vec<&str> = header_value.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return Err(SecurityError::InvalidAuthScheme);
    }
    if !parts[0].eq_ignore_ascii_case("Bearer") {
        return Err(SecurityError::InvalidAuthScheme);
    }
    Ok(parts[1])
}

/// Axum extractor implementation for `AuthenticatedUser`.
///
/// This extracts the JWT from the `Authorization: Bearer <token>` header,
/// validates it using the `JwtValidator` from the application state,
/// and returns an `AuthenticatedUser` on success.
///
/// The application state must implement `FromRef<S>` for `Arc<JwtValidator>`.
///
/// # Example
///
/// ```ignore
/// async fn protected_handler(user: AuthenticatedUser) -> impl IntoResponse {
///     format!("Hello, {}!", user.sub)
/// }
/// ```
impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
    Arc<JwtValidator>: axum::extract::FromRef<S>,
{
    type Rejection = quarlus_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Extract the Authorization header
        let auth_header = parts.headers.get(AUTHORIZATION).ok_or_else(|| {
            warn!(uri = %parts.uri, "Missing Authorization header");
            SecurityError::MissingAuthHeader
        })?;

        let auth_value = auth_header
            .to_str()
            .map_err(|_| SecurityError::InvalidAuthScheme)?;

        // 2. Extract the Bearer token
        let token = extract_bearer_token(auth_value)?;

        // 3. Get the JwtValidator from state
        let validator: Arc<JwtValidator> = Arc::from_ref(state);

        // 4. Validate the token
        let user: AuthenticatedUser = validator.validate(token).await.map_err(|e| {
            warn!(uri = %parts.uri, error = %e, "JWT validation failed");
            quarlus_core::AppError::from(e)
        })?;

        debug!(sub = %user.sub, "Authenticated request");
        Ok(user)
    }
}

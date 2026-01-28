use std::sync::Arc;

use quarlus_core::http::extract::{FromRef, FromRequestParts};
use quarlus_core::http::header::{Parts, AUTHORIZATION};
use tracing::{debug, warn};

use crate::error::SecurityError;
use crate::identity::{AuthenticatedUser, IdentityBuilder};
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

/// Extract and validate a JWT identity from request parts.
///
/// This is the shared extraction logic used by [`AuthenticatedUser`]'s
/// `FromRequestParts` implementation. Use it to implement `FromRequestParts`
/// for your own identity type backed by a custom [`IdentityBuilder`].
///
/// # Example
///
/// ```ignore
/// impl<S> FromRequestParts<S> for DbUser
/// where
///     S: Send + Sync,
///     Arc<JwtValidator<DbIdentityBuilder>>: FromRef<S>,
/// {
///     type Rejection = quarlus_core::AppError;
///
///     async fn from_request_parts(
///         parts: &mut Parts,
///         state: &S,
///     ) -> Result<Self, Self::Rejection> {
///         quarlus_security::extract_jwt_identity::<S, DbIdentityBuilder>(parts, state).await
///     }
/// }
/// ```
pub async fn extract_jwt_identity<S, B>(
    parts: &mut Parts,
    state: &S,
) -> Result<B::Identity, quarlus_core::AppError>
where
    S: Send + Sync,
    B: IdentityBuilder + 'static,
    Arc<JwtValidator<B>>: FromRef<S>,
{
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
    let validator: Arc<JwtValidator<B>> = Arc::from_ref(state);

    // 4. Validate the token and build the identity
    let identity = validator.validate(token).await.map_err(|e| {
        warn!(uri = %parts.uri, error = %e, "JWT validation failed");
        quarlus_core::AppError::from(e)
    })?;

    debug!(uri = %parts.uri, "Authenticated request");
    Ok(identity)
}

/// Axum extractor implementation for `AuthenticatedUser`.
///
/// This extracts the JWT from the `Authorization: Bearer <token>` header,
/// validates it using the `JwtValidator` from the application state,
/// and returns an `AuthenticatedUser` on success.
///
/// The application state must implement `FromRef<S>` for `Arc<JwtValidator>`.
///
/// For custom identity types, use [`extract_jwt_identity`] to implement
/// `FromRequestParts` with minimal boilerplate.
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
    Arc<JwtValidator>: quarlus_core::http::extract::FromRef<S>,
{
    type Rejection = quarlus_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        extract_jwt_identity::<S, crate::identity::DefaultIdentityBuilder>(parts, state).await
    }
}

use std::sync::Arc;

use r2e_core::http::extract::{FromRef, FromRequestParts, OptionalFromRequestParts};
use r2e_core::http::header::{Parts, AUTHORIZATION};
use tracing::{debug, warn};

use crate::error::SecurityError;
use crate::identity::{AuthenticatedUser, IdentityBuilder};
use crate::jwt::{JwtClaimsValidator, JwtValidator};

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

/// Extract the Bearer token from request headers.
///
/// Returns the raw token string without validation.
pub fn extract_bearer_token_from_parts(parts: &Parts) -> Result<&str, SecurityError> {
    let auth_header = parts.headers.get(AUTHORIZATION).ok_or_else(|| {
        warn!(uri = %parts.uri, "Missing Authorization header");
        SecurityError::MissingAuthHeader
    })?;

    let auth_value = auth_header
        .to_str()
        .map_err(|_| SecurityError::InvalidAuthScheme)?;

    extract_bearer_token(auth_value)
}

/// Extract and validate JWT claims from request parts.
///
/// This is the low-level extraction function that validates the JWT and returns
/// raw claims. Use this when implementing custom identity types that need
/// additional processing (e.g., database lookup).
///
/// # Example: Custom identity with database lookup
///
/// ```ignore
/// use r2e_security::{extract_jwt_claims, JwtClaimsValidator, AuthenticatedUser};
/// use r2e_core::http::extract::{FromRef, FromRequestParts};
///
/// pub struct DbUser {
///     pub claims: AuthenticatedUser,
///     pub profile: UserProfile,
/// }
///
/// impl<S> FromRequestParts<S> for DbUser
/// where
///     S: Send + Sync,
///     Arc<JwtClaimsValidator>: FromRef<S>,
///     SqlitePool: FromRef<S>,
/// {
///     type Rejection = r2e_core::AppError;
///
///     async fn from_request_parts(
///         parts: &mut Parts,
///         state: &S,
///     ) -> Result<Self, Self::Rejection> {
///         // 1. Validate JWT and get claims
///         let claims = extract_jwt_claims(parts, state).await?;
///         let sub = claims["sub"].as_str().unwrap_or_default();
///
///         // 2. Build light identity from claims
///         let authenticated = AuthenticatedUser::from_claims(claims);
///
///         // 3. Database lookup (only for this identity type)
///         let pool = SqlitePool::from_ref(state);
///         let profile = sqlx::query_as!(UserProfile, "SELECT * FROM users WHERE sub = ?", sub)
///             .fetch_one(&pool)
///             .await
///             .map_err(|e| r2e_core::AppError::internal(e.to_string()))?;
///
///         Ok(DbUser { claims: authenticated, profile })
///     }
/// }
/// ```
///
/// Now you can use both identity types in your controllers:
///
/// ```ignore
/// #[get("/light")]
/// async fn light(&self, user: AuthenticatedUser) -> Json<...> {
///     // No database round-trip
/// }
///
/// #[get("/full")]
/// async fn full(&self, user: DbUser) -> Json<...> {
///     // With database lookup
/// }
/// ```
pub async fn extract_jwt_claims<S>(
    parts: &Parts,
    state: &S,
) -> Result<serde_json::Value, r2e_core::AppError>
where
    S: Send + Sync,
    Arc<JwtClaimsValidator>: FromRef<S>,
{
    let token = extract_bearer_token_from_parts(parts)?;
    let validator: Arc<JwtClaimsValidator> = Arc::from_ref(state);

    let claims = validator.validate(token).await.map_err(|e| {
        warn!(uri = %parts.uri, error = %e, "JWT validation failed");
        r2e_core::AppError::from(e)
    })?;

    debug!(uri = %parts.uri, "JWT claims extracted");
    Ok(claims)
}

/// Extract and validate a JWT identity from request parts.
///
/// This is the shared extraction logic used by [`AuthenticatedUser`]'s
/// `FromRequestParts` implementation. Use it to implement `FromRequestParts`
/// for your own identity type backed by a custom [`IdentityBuilder`].
///
/// For custom identities that need additional processing (like database lookups),
/// prefer using [`extract_jwt_claims`] instead, which gives you access to raw
/// claims for custom handling.
///
/// # Example
///
/// ```ignore
/// impl<S> FromRequestParts<S> for MyUser
/// where
///     S: Send + Sync,
///     Arc<JwtValidator<MyIdentityBuilder>>: FromRef<S>,
/// {
///     type Rejection = r2e_core::AppError;
///
///     async fn from_request_parts(
///         parts: &mut Parts,
///         state: &S,
///     ) -> Result<Self, Self::Rejection> {
///         extract_jwt_identity::<S, MyIdentityBuilder>(parts, state).await
///     }
/// }
/// ```
pub async fn extract_jwt_identity<S, B>(
    parts: &Parts,
    state: &S,
) -> Result<B::Identity, r2e_core::AppError>
where
    S: Send + Sync,
    B: IdentityBuilder + 'static,
    Arc<JwtValidator<B>>: FromRef<S>,
{
    let token = extract_bearer_token_from_parts(parts)?;
    let validator: Arc<JwtValidator<B>> = Arc::from_ref(state);

    let identity = validator.validate(token).await.map_err(|e| {
        warn!(uri = %parts.uri, error = %e, "JWT validation failed");
        r2e_core::AppError::from(e)
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
/// The application state must provide either:
/// - `Arc<JwtValidator>` via `FromRef` (recommended for simple cases)
/// - `Arc<JwtClaimsValidator>` via `FromRef` (for multiple identity types)
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
    Arc<JwtClaimsValidator>: FromRef<S>,
{
    type Rejection = r2e_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let claims = extract_jwt_claims(parts, state).await?;
        Ok(AuthenticatedUser::from_claims(claims))
    }
}

/// Optional extractor for `AuthenticatedUser`.
///
/// Enables `Option<AuthenticatedUser>` as a handler parameter for endpoints
/// that work both with and without authentication:
///
/// - No `Authorization` header → `Ok(None)`
/// - Valid JWT → `Ok(Some(user))`
/// - Invalid/expired JWT → `Err(AppError::Unauthorized)`
///
/// # Example
///
/// ```ignore
/// #[get("/whoami")]
/// async fn whoami(
///     &self,
///     #[inject(identity)] user: Option<AuthenticatedUser>,
/// ) -> Json<String> {
///     match user {
///         Some(u) => Json(format!("Hello, {}", u.sub())),
///         None => Json("Hello, anonymous".to_string()),
///     }
/// }
/// ```
impl<S> OptionalFromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
    Arc<JwtClaimsValidator>: FromRef<S>,
{
    type Rejection = r2e_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        if !parts.headers.contains_key(AUTHORIZATION) {
            return Ok(None);
        }

        let claims = extract_jwt_claims(parts, state).await?;
        Ok(Some(AuthenticatedUser::from_claims(claims)))
    }
}

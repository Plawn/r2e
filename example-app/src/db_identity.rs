//! Demonstrates multiple identity types from the same JWT validator.
//!
//! This module shows two patterns:
//!
//! 1. **Light identity** (`AuthenticatedUser`) — just JWT claims, no DB round-trip
//! 2. **Full identity** (`DbUser`) — JWT claims + database lookup
//!
//! Both use the same `JwtClaimsValidator`, so the JWT is validated only once.

use std::sync::Arc;

use quarlus_core::http::extract::{FromRef, FromRequestParts};
use quarlus_core::http::header::Parts;
use quarlus_core::Identity;
use quarlus_security::{extract_jwt_claims, AuthenticatedUser, JwtClaimsValidator};
use serde::{Deserialize, Serialize};

/// A database-backed user identity.
///
/// Unlike [`AuthenticatedUser`] which only contains raw JWT claims,
/// `DbUser` includes the full user profile fetched from the database.
///
/// Use this when you need user data that isn't in the JWT (e.g., preferences,
/// profile picture, subscription status).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbUser {
    /// The light identity (JWT claims only)
    pub auth: AuthenticatedUser,
    /// Database profile data
    pub profile: UserProfile,
}

/// User profile data from the database.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl Identity for DbUser {
    fn sub(&self) -> &str {
        self.auth.sub()
    }
    fn roles(&self) -> &[String] {
        self.auth.roles()
    }
}

impl DbUser {
    /// Access the underlying authenticated user (JWT claims).
    pub fn auth(&self) -> &AuthenticatedUser {
        &self.auth
    }

    /// Access the database profile.
    pub fn profile(&self) -> &UserProfile {
        &self.profile
    }
}

/// Axum extractor for `DbUser`.
///
/// This extracts and validates the JWT using `JwtClaimsValidator`,
/// then performs a database lookup to fetch the user profile.
///
/// The state must provide:
/// - `Arc<JwtClaimsValidator>` for JWT validation
/// - `sqlx::SqlitePool` for database access
impl<S> FromRequestParts<S> for DbUser
where
    S: Send + Sync,
    Arc<JwtClaimsValidator>: FromRef<S>,
    sqlx::SqlitePool: FromRef<S>,
{
    type Rejection = quarlus_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Validate JWT and get claims (same validation as AuthenticatedUser)
        let claims = extract_jwt_claims(parts, state).await?;
        let sub = claims["sub"].as_str().unwrap_or_default().to_owned();

        // 2. Build light identity from claims
        let auth = AuthenticatedUser::from_claims(claims);

        // 3. Database lookup for profile data
        let pool = sqlx::SqlitePool::from_ref(state);
        let row: Option<(i64, String, String)> =
            sqlx::query_as("SELECT id, name, email FROM users WHERE sub = ?")
                .bind(&sub)
                .fetch_optional(&pool)
                .await
                .map_err(|e| quarlus_core::AppError::Internal(format!("DB error: {e}")))?;

        let profile = row
            .map(|(id, name, email)| UserProfile { id, name, email })
            .ok_or_else(|| {
                quarlus_core::AppError::NotFound(format!("No user profile for sub '{sub}'"))
            })?;

        Ok(DbUser { auth, profile })
    }
}

//! Demonstrates multiple identity types from the same JWT validator.
//!
//! This module shows two patterns:
//!
//! 1. **Light identity** (`AuthenticatedUser`) — just JWT claims, no DB round-trip
//! 2. **Full identity** (`DbUser`) — JWT claims + database lookup
//!
//! Both use the same `JwtClaimsValidator`, so the JWT is validated only once.
//!
//! `DbUser` uses the [`ClaimsIdentity`] trait and [`impl_claims_identity_extractor!`]
//! macro to reduce boilerplate. Compare with the manual `FromRequestParts` approach
//! documented in `r2e-security/src/extractor.rs`.

use r2e::http::extract::FromRef;
use r2e::Identity;
use r2e::r2e_security::{
    impl_claims_identity_extractor, AuthenticatedUser, ClaimsIdentity, RoleBasedIdentity,
};
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
    fn email(&self) -> Option<&str> {
        self.auth.email()
    }
    fn claims(&self) -> Option<&serde_json::Value> {
        self.auth.claims()
    }
}

impl RoleBasedIdentity for DbUser {
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

impl<S> ClaimsIdentity<S> for DbUser
where
    S: Send + Sync,
    sqlx::SqlitePool: FromRef<S>,
{
    async fn from_jwt_claims(
        claims: serde_json::Value,
        state: &S,
    ) -> Result<Self, r2e::HttpError> {
        let sub = claims["sub"].as_str().unwrap_or_default().to_owned();
        let auth = AuthenticatedUser::from_claims(claims);

        let pool = sqlx::SqlitePool::from_ref(state);
        let row: Option<(i64, String, String)> =
            sqlx::query_as("SELECT id, name, email FROM users WHERE sub = ?")
                .bind(&sub)
                .fetch_optional(&pool)
                .await
                .map_err(|e| r2e::HttpError::Internal(format!("DB error: {e}")))?;

        let profile = row
            .map(|(id, name, email)| UserProfile { id, name, email })
            .ok_or_else(|| {
                r2e::HttpError::NotFound(format!("No user profile for sub '{sub}'"))
            })?;

        Ok(DbUser { auth, profile })
    }
}

impl_claims_identity_extractor!(DbUser);

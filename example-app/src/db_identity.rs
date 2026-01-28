//! Demonstrates a database-backed identity using `IdentityBuilder`.
//!
//! Instead of using `AuthenticatedUser` (which only contains JWT claims),
//! this module fetches the full user entity from the database during
//! JWT validation, making it directly available in controllers.

use std::sync::Arc;

use quarlus_core::http::extract::{FromRef, FromRequestParts};
use quarlus_core::http::header::Parts;
use quarlus_core::Identity;
use quarlus_security::{extract_jwt_identity, IdentityBuilder, JwtValidator, SecurityError};
use serde::{Deserialize, Serialize};

/// A database-backed user identity.
///
/// Unlike [`AuthenticatedUser`](quarlus_security::AuthenticatedUser) which only
/// contains raw JWT claims, `DbUser` is the full user entity fetched from the
/// database using the JWT `sub` claim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbUser {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub sub: String,
    pub roles: Vec<String>,
}

impl Identity for DbUser {
    fn sub(&self) -> &str {
        &self.sub
    }
    fn roles(&self) -> &[String] {
        &self.roles
    }
}

/// Identity builder that fetches the user from the database by JWT `sub` claim.
pub struct DbIdentityBuilder {
    pool: sqlx::SqlitePool,
}

impl DbIdentityBuilder {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }
}

impl IdentityBuilder for DbIdentityBuilder {
    type Identity = DbUser;

    fn build(
        &self,
        claims: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<DbUser, SecurityError>> + Send {
        let pool = self.pool.clone();
        async move {
            let sub = claims
                .get("sub")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let roles: Vec<String> = claims
                .get("roles")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| r.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let row: (i64, String, String) =
                sqlx::query_as("SELECT id, name, email FROM users WHERE sub = ?")
                    .bind(sub)
                    .fetch_optional(&pool)
                    .await
                    .map_err(|e| SecurityError::ValidationFailed(format!("DB error: {e}")))?
                    .ok_or_else(|| {
                        SecurityError::ValidationFailed(format!(
                            "No user found for sub '{sub}'"
                        ))
                    })?;

            Ok(DbUser {
                id: row.0,
                name: row.1,
                email: row.2,
                sub: sub.to_owned(),
                roles,
            })
        }
    }
}

/// Axum extractor — delegates to [`extract_jwt_identity`] with the DB builder.
impl<S> FromRequestParts<S> for DbUser
where
    S: Send + Sync,
    Arc<JwtValidator<DbIdentityBuilder>>: FromRef<S>,
{
    type Rejection = quarlus_core::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        extract_jwt_identity::<S, DbIdentityBuilder>(parts, state).await
    }
}

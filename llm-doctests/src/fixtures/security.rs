//! Scaffolding for `llm/security.md`.

use r2e::prelude::*;

pub use std::collections::HashMap;
pub use std::sync::Arc;

pub use serde::{Deserialize, Serialize};
pub use serde_json::{json, Value};
pub use sqlx::SqlitePool;

/// Custom-identity glue the snippets name without re-importing it every time.
pub use r2e::r2e_security::{
    impl_claims_identity_extractor, FromValidatedJwtClaims, JwtClaimSet, RoleBasedIdentity,
};

/// Entities returned by the controller snippets.
#[derive(Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub email: String,
}

#[derive(Clone, Serialize)]
pub struct Profile {
    pub sub: String,
}

#[derive(Clone, Serialize)]
pub struct Plan {
    pub name: String,
}

/// The profile row a custom identity loads from the database.
#[derive(Clone, Serialize)]
pub struct UserProfile {
    pub display_name: String,
}

impl UserProfile {
    pub async fn load(_pool: &SqlitePool, _sub: &str) -> Result<Self, HttpError> {
        todo!()
    }
}

/// Services the controllers inject.
#[derive(Clone)]
pub struct AccountService;

impl AccountService {
    pub async fn profile(&self, sub: &str) -> Profile {
        Profile { sub: sub.into() }
    }

    pub async fn plans(&self) -> Vec<Plan> {
        Vec::new()
    }
}

#[derive(Clone)]
pub struct UserService;

impl UserService {
    pub async fn list(&self) -> Vec<User> {
        Vec::new()
    }
}

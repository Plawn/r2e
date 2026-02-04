//! Demonstrates the difference between light and full identity types.
//!
//! Both endpoints use the **same** `JwtClaimsValidator` — the JWT is validated once.
//! The difference is what happens after validation:
//!
//! - `/identity/light` — returns JWT claims only (no DB round-trip)
//! - `/identity/full` — returns JWT claims + database profile (1 DB query)

use quarlus::prelude::*;
use quarlus::quarlus_security::AuthenticatedUser;

use crate::db_identity::DbUser;
use crate::state::Services;

/// Controller comparing light vs full identity extraction.
///
/// This is a "mixed" controller using parameter-level identity injection,
/// allowing different identity types per endpoint.
#[derive(Controller)]
#[controller(path = "/identity", state = Services)]
pub struct IdentityController;

#[routes]
impl IdentityController {
    /// Light identity — JWT claims only, no database round-trip.
    ///
    /// Use this when you only need the user's `sub`, `email`, or `roles`
    /// from the JWT token. Fast and stateless.
    ///
    /// Response example:
    /// ```json
    /// {
    ///   "sub": "user-123",
    ///   "email": "demo@quarlus.dev",
    ///   "roles": ["user", "admin"],
    ///   "claims": { ... }
    /// }
    /// ```
    ///
    /// ```bash
    /// curl -H "Authorization: Bearer $TOKEN" http://localhost:3001/identity/light
    /// ```
    #[get("/light")]
    async fn light(
        &self,
        user: AuthenticatedUser,
    ) -> Json<AuthenticatedUser> {
        // No DB query — just the JWT claims
        Json(user)
    }

    /// Full identity — JWT claims + database profile.
    ///
    /// Use this when you need user data that isn't in the JWT
    /// (e.g., display name, preferences, subscription status).
    ///
    /// Response example:
    /// ```json
    /// {
    ///   "auth": {
    ///     "sub": "user-123",
    ///     "email": "demo@quarlus.dev",
    ///     "roles": ["user", "admin"],
    ///     "claims": { ... }
    ///   },
    ///   "profile": {
    ///     "id": 1,
    ///     "name": "Alice",
    ///     "email": "alice@example.com"
    ///   }
    /// }
    /// ```
    ///
    /// ```bash
    /// curl -H "Authorization: Bearer $TOKEN" http://localhost:3001/identity/full
    /// ```
    #[get("/full")]
    async fn full(
        &self,
         user: DbUser,
    ) -> Json<DbUser> {
        // Includes DB lookup for user profile
        Json(user)
    }
}

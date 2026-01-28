use quarlus_core::http::Json;
use quarlus_core::prelude::*;

use crate::db_identity::DbUser;
use crate::state::Services;

/// Controller demonstrating database-backed identity.
///
/// Uses `DbUser` instead of `AuthenticatedUser` — the identity is
/// resolved from the database during JWT extraction, giving direct
/// access to the full user entity (id, name, email) rather than
/// just raw JWT claims.
#[derive(Controller)]
#[controller(path = "/db-identity", state = Services)]
pub struct DbIdentityController {
    #[inject(identity)]
    user: DbUser,
}

#[routes]
impl DbIdentityController {
    /// Returns the full database user entity resolved from the JWT `sub` claim.
    #[get("/me")]
    async fn me(&self) -> Json<DbUser> {
        Json(self.user.clone())
    }
}

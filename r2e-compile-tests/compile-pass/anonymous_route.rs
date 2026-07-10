//! `#[anonymous]` (task #643): per-route opt-out of the controller's
//! struct-level identity. Unmarked routes stay authenticated (fail-closed);
//! marked routes run on the core with no identity extraction.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/posts")]
pub struct PostController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl PostController {
    /// Public — no JWT extraction; injected/config fields remain reachable.
    #[get("/")]
    #[anonymous]
    async fn list(&self) -> Json<Vec<String>> {
        Json(vec![])
    }

    /// Guards without identity needs (e.g. rate limits) are fine on
    /// anonymous routes; they observe `identity: None`.
    #[get("/health")]
    #[anonymous]
    #[intercept(r2e::r2e_utils::Logged::info())]
    async fn health(&self) -> Json<&'static str> {
        Json("ok")
    }

    /// Authenticated by default — reads the struct identity.
    #[post("/")]
    async fn create(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }

    /// Roles keep working off the struct identity on unmarked routes.
    #[get("/audit")]
    #[roles("admin")]
    async fn audit(&self) -> Json<&'static str> {
        Json("audit")
    }

    /// Adaptive public route: `#[anonymous]` + an *optional* identity
    /// parameter — personalized when a valid credential is present, never 401.
    #[get("/feed")]
    #[anonymous]
    async fn feed(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> Json<String> {
        Json(user.map(|u| u.sub().to_string()).unwrap_or_default())
    }
}

fn main() {}

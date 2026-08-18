//! A controller-level identity-requiring guard applies to EVERY non-anonymous
//! route, so the per-route rule must hold universally: one route with its own
//! `#[inject(identity)]` param must not legalize a sibling route where the
//! guard can only ever see `None` (it would 401 in production instead of
//! failing the build). Codex review finding on task #906.

use r2e::prelude::*;
use r2e::r2e_rate_limit::RateLimit;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/things")]
pub struct ThingController;

#[routes]
#[guard(RateLimit::per_user(5, 60))]
impl ThingController {
    #[get("/mine")]
    async fn mine(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<String> {
        Json(user.sub().to_string())
    }

    // No identity param and no struct identity: the controller-level per-user
    // guard could never key a bucket here.
    #[get("/")]
    async fn list(&self) -> Json<&'static str> {
        Json("things")
    }
}

fn main() {}

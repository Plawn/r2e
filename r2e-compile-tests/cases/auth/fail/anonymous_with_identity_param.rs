//! `#[anonymous]` + an `#[inject(identity)]` parameter is contradictory: the
//! marker opts out of authentication, the parameter requires it.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/test")]
pub struct MyController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl MyController {
    #[get("/")]
    #[anonymous]
    async fn show(&self, #[inject(identity)] other: AuthenticatedUser) -> Json<String> {
        Json(other.sub().to_string())
    }
}

fn main() {}

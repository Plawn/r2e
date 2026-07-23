//! `#[anonymous]` needs a fail-closed baseline to opt out of. An `Option<T>`
//! struct identity never rejects — the routes are already public — so the
//! marker is rejected.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/test")]
pub struct MyController {
    #[inject(identity)]
    user: Option<AuthenticatedUser>,
}

#[routes]
impl MyController {
    #[get("/")]
    #[anonymous]
    async fn show(&self) -> Json<String> {
        Json("already public".to_string())
    }
}

fn main() {}

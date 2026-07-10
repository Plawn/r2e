//! `#[anonymous]` + `#[roles]` is contradictory: role checks need an
//! authenticated identity, an anonymous route has none.

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
    #[roles("admin")]
    async fn show(&self) -> Json<String> {
        Json("never".to_string())
    }
}

fn main() {}

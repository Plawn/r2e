//! An `#[anonymous]` route runs on the controller core, where the identity
//! field does not exist — reading it is a compile error (same diagnostic as
//! consumers/scheduled methods touching request-scoped fields).

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
    async fn show(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }
}

fn main() {}

//! `#[anonymous]` skips identity extraction, so guards on that route run with
//! `identity: None`. An identity-requiring guard (`FgaCheck`) there could only
//! ever 401 — rejected at compile time. To opt back in, the route must declare
//! its own `Option<..>` identity parameter (adaptive route).

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/docs")]
pub struct DocController {
    #[inject(identity)]
    _user: AuthenticatedUser,
}

#[routes]
impl DocController {
    #[get("/{id}")]
    #[anonymous]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("id"))]
    async fn show(&self, Path(_id): Path<String>) -> Json<String> {
        Json("doc".to_string())
    }
}

fn main() {}

//! Allowed: an FGA guard on a controller with a required struct-level identity.
//! `guard_identity` yields `Some(&user)`, so the identity requirement is met.

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
    #[guard(FgaCheck::relation("viewer").on("document").from_path("id"))]
    async fn show(&self, Path(_id): Path<String>) -> Json<String> {
        Json("doc".to_string())
    }
}

fn main() {}

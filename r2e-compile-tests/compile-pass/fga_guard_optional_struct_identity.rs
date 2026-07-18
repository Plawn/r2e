//! Allowed: an FGA guard on a controller with an `Option<..>` struct identity.
//! `guard_identity` may still be `Some` at runtime, so the compile-time check
//! passes; the runtime `None` → 401 in `FgaGuard::check` stays as the backstop.

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/docs")]
pub struct DocController {
    #[inject(identity)]
    _user: Option<AuthenticatedUser>,
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

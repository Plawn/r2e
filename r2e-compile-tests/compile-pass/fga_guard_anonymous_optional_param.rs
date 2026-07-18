//! Allowed: an `#[anonymous]` route on a required-struct-identity controller
//! that opts back into identity with an `Option<..>` identity parameter. The
//! guard's identity comes from that param (adaptive route: may be `Some`), so
//! the identity-requiring FGA guard is accepted.

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
    async fn show(
        &self,
        #[inject(identity)] _user: Option<AuthenticatedUser>,
        Path(_id): Path<String>,
    ) -> Json<String> {
        Json("doc".to_string())
    }
}

fn main() {}

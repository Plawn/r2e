//! Allowed: an FGA guard on a route with a parameter-level identity, even
//! though the controller declares no struct identity. The guard's identity
//! comes from the route's `#[inject(identity)]` param, so it can be `Some`.

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/docs")]
pub struct DocController;

#[routes]
impl DocController {
    #[get("/{id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("id"))]
    async fn show(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
        Path(_id): Path<String>,
    ) -> Json<String> {
        Json("doc".to_string())
    }
}

fn main() {}

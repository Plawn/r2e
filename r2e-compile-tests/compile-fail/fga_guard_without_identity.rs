//! An FGA guard checks `user:{identity.sub()}`, so it can only ever succeed
//! with an authenticated identity. On a controller with no identity (no
//! struct-level `#[inject(identity)]`, no identity parameter) the check is
//! statically always `None` → a guaranteed 401. `FgaCheck` declares
//! `DecoratorSpec::REQUIRES_IDENTITY = true`, so `#[routes]` rejects the
//! placement at compile time instead of leaving it to production.

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;

#[controller(path = "/docs")]
pub struct DocController;

#[routes]
impl DocController {
    #[get("/{id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("id"))]
    async fn show(&self, Path(_id): Path<String>) -> Json<String> {
        Json("doc".to_string())
    }
}

fn main() {}

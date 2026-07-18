//! An FGA `from_path("...")` guard that references a path parameter which is not
//! declared anywhere in the route (neither the method path nor the controller
//! prefix) is a compile error — previously this only failed at runtime with an
//! `ObjectResolutionFailed` ("path param not found").

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

// The identity satisfies the guard's `REQUIRES_IDENTITY` so this fixture
// exercises ONLY the `from_path` placeholder mismatch.
#[controller(path = "/documents")]
pub struct DocController {
    #[inject(identity)]
    _user: AuthenticatedUser,
}

#[routes]
impl DocController {
    // The route declares `{doc_id}`, but the guard references `document_id`.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("document_id"))]
    async fn get_doc(&self, Path(_doc_id): Path<String>) -> Json<String> {
        Json(String::new())
    }
}

fn main() {}

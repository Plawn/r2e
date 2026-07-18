//! Document controller demonstrating the idiomatic OpenFGA guard path.
//!
//! The struct-level `#[inject(identity)]` makes every route fail-closed (a
//! missing/invalid token is a 401). Each route then adds an `FgaCheck` guard:
//! the guard forms `user:{sub}` from the identity, resolves the object as
//! `document:{doc_id}` from the `{doc_id}` path parameter, and asks OpenFGA
//! whether that relation holds. A denied check returns 403.
//!
//! The literal `from_path("doc_id")` is compile-checked against the route's
//! `{doc_id}` placeholder.

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl DocumentController {
    /// Read a document — requires the `viewer` relation.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("doc_id"))]
    async fn view(&self, Path(doc_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": doc_id,
            "user": self.user.sub,
            "action": "view",
        }))
    }

    /// Edit a document — requires the `editor` relation.
    #[put("/{doc_id}")]
    #[guard(FgaCheck::relation("editor").on("document").from_path("doc_id"))]
    async fn edit(&self, Path(doc_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": doc_id,
            "user": self.user.sub,
            "action": "edit",
        }))
    }
}

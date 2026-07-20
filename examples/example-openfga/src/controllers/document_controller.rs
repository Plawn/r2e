//! Document controller demonstrating the idiomatic OpenFGA guard path.
//!
//! The struct-level `#[inject(identity)]` makes every route fail-closed (a
//! missing/invalid token is a 401). Each route then adds an `FgaCheck` guard:
//! the guard forms `user:{sub}` from the identity, resolves the object as
//! `document:{doc_id}` from the `{doc_id}` path parameter, and asks OpenFGA
//! whether that relation holds. A denied check returns 403.
//!
//! Everything in the guard expression is compile-checked: the relation and
//! object type against the checked-in `fga/model.fga` (`authz::…`, generated
//! by `model!`), the path parameter against the route's `{doc_id}`
//! placeholder (`path::doc_id`).

use crate::authz;
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
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": doc_id,
            "user": self.user.sub,
            "action": "view",
        }))
    }

    /// Edit a document — requires the `editor` relation.
    #[put("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::editor).from_path(path::doc_id))]
    async fn edit(&self, Path(doc_id): Path<String>) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "id": doc_id,
            "user": self.user.sub,
            "action": "edit",
        }))
    }
}

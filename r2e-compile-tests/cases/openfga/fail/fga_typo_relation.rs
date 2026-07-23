//! A typo'd relation in `FgaCheck::has` is an unresolved name at the guard
//! site — not a stringly-typed permanent 403.

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use r2e::r2e_security::AuthenticatedUser;

r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1

type user

type document
  relations
    define viewer: [user]
"#);

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl DocumentController {
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viwer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> String {
        doc_id
    }
}

fn main() {}

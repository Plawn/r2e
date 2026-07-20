//! `model!` generates a typed API from the `.fga` model: relation consts
//! usable with `FgaCheck::has`, typed object constructors, and
//! `DirectlyAssignable` impls mirroring `directly_related_user_types`.
//! (Inline DSL here — trybuild fixtures have no stable manifest dir; the
//! file-path form is exercised by `examples/example-openfga`.)

use r2e::prelude::*;
use r2e::r2e_openfga::typed::{DirectlyAssignable, WildcardOf};
use r2e::r2e_openfga::FgaCheck;
use r2e::r2e_security::AuthenticatedUser;

r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define parent: [document]
    define viewer: [user, user:*, team#member] or viewer from parent
    define editor: [user] and viewer
"#);

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl DocumentController {
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> String {
        doc_id
    }
}

fn assert_assignable<S, R: DirectlyAssignable<S>>() {}

fn main() {
    // MODEL is the boot-time apply/verify payload.
    let json: serde_json::Value = serde_json::from_str(authz::MODEL).unwrap();
    assert_eq!(json["schema_version"], "1.1");

    // Typed objects format `type:id`.
    let doc = authz::document::id("readme");
    assert_eq!(doc.as_str(), "document:readme");
    assert!(authz::document::try_id("a:b").is_err());

    // Relation consts carry relation + object type.
    assert_eq!(authz::document::viewer.name(), "viewer");
    assert_eq!(authz::document::viewer.object_type(), "document");

    // Userset / wildcard subjects.
    let set = authz::team::member.of(authz::team::id("eng"));
    assert_eq!(set.as_str(), "team:eng#member");
    assert_eq!(authz::user::wildcard().to_string(), "user:*");

    // `[user, user:*, team#member]` on viewer — all three subject markers.
    assert_assignable::<authz::user::Ty, authz::document::Viewer>();
    assert_assignable::<WildcardOf<authz::user::Ty>, authz::document::Viewer>();
    assert_assignable::<(authz::team::Ty, authz::team::Member), authz::document::Viewer>();
    // `[user]` on editor.
    assert_assignable::<authz::user::Ty, authz::document::Editor>();
}

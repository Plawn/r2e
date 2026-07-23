//! `FgaClient::grant` is bounded on `DirectlyAssignable`: granting a subject
//! type the model does not list in the relation's
//! `directly_related_user_types` is a compile error. Here `editor` only
//! allows `[user]`, so a `team#member` userset subject is rejected.

use r2e::r2e_openfga::{FgaClient, MockBackend, OpenFgaRegistry};

r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define viewer: [user, team#member]
    define editor: [user]
"#);

async fn exercise(fga: &FgaClient) {
    let doc = authz::document::id("readme");
    let eng_members = authz::team::member.of(authz::team::id("eng"));

    fga.grant(&eng_members, authz::document::editor, &doc).await.unwrap();
}

fn main() {
    let fga = FgaClient::new(OpenFgaRegistry::new(MockBackend::new()));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(exercise(&fga));
}

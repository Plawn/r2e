//! `FgaClient` typed API compiles for every subject shape the model allows:
//! direct user, userset (`team#member`), and public wildcard (`user:*`) on
//! `viewer`; direct user on `editor`. `check` needs no `DirectlyAssignable`
//! bound (checks may target computed relations).

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
    define viewer: [user, user:*, team#member] or editor
    define editor: [user]
"#);

async fn exercise(fga: &FgaClient) {
    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    fga.grant(&alice, authz::document::viewer, &doc).await.unwrap();
    fga.grant(&alice, authz::document::editor, &doc).await.unwrap();
    fga.grant(
        &authz::team::member.of(authz::team::id("eng")),
        authz::document::viewer,
        &doc,
    )
    .await
    .unwrap();
    fga.grant(&authz::user::wildcard(), authz::document::viewer, &doc)
        .await
        .unwrap();

    // `viewer` is partly computed (`or editor`) — still checkable.
    let _: bool = fga.check(&alice, authz::document::viewer, &doc).await.unwrap();

    fga.revoke(&alice, authz::document::viewer, &doc).await.unwrap();
}

fn main() {
    let fga = FgaClient::new(OpenFgaRegistry::new(MockBackend::new()));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(exercise(&fga));
}

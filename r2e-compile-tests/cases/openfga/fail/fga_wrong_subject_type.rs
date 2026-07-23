//! `DirectlyAssignable` mirrors the model's `directly_related_user_types`:
//! `editor` only allows `[user]`, so a `team#member` subject marker does not
//! satisfy the bound (this is what the typed write API checks on `grant`).

use r2e::r2e_openfga::typed::DirectlyAssignable;

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

fn assert_assignable<S, R: DirectlyAssignable<S>>() {}

fn main() {
    assert_assignable::<(authz::team::Ty, authz::team::Member), authz::document::Editor>();
}

//! An uppercase-first relation would make the generated lowercase-const
//! convention collide with the relation's own marker struct — rejected with
//! a friendly `model!` error instead of a cryptic E0428.

r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1

type user

type document
  relations
    define Viewer: [user]
"#);

fn main() {}

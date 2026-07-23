//! A semantically invalid `.fga` (here: `editor` referenced but never
//! defined) fails at the `model!` invocation with the offending relation.

r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1

type user

type document
  relations
    define viewer: [user] or editor
"#);

fn main() {}

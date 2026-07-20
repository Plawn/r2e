//! Proc macros for `r2e-openfga`. See `model!`.

use proc_macro::TokenStream;

mod model_macro;

/// Generate a typed authorization API from a checked-in `.fga` model file.
///
/// ```ignore
/// r2e_openfga::model!(pub mod authz = "fga/model.fga");
/// ```
///
/// The path is relative to the crate root (`CARGO_MANIFEST_DIR`). The file is
/// parsed and semantically validated at compile time; errors point at the
/// macro invocation with the offending `.fga` line in the message.
///
/// For tests and tiny models the DSL can be given inline instead of a path:
///
/// ```ignore
/// r2e_openfga::model!(pub mod authz = inline r#"
/// model
///   schema 1.1
/// type user
/// "#);
/// ```
///
/// The generated module contains:
/// - `authz::MODEL` — the schema 1.1 JSON of the model (for boot-time
///   apply/verify);
/// - per type: `authz::<type>::Ty` (an [`FgaType`] marker) and
///   `authz::<type>::id("x")` → `FgaObject<Ty>` (formats `type:x`, rejects
///   `:` in the id);
/// - per relation: `authz::<type>::<relation>` — an `FgaRel<Ty, Marker>`
///   const usable with `FgaCheck::has(...)`; a typo is a compile error;
/// - `DirectlyAssignable` impls encoding `directly_related_user_types`, so
///   typed writes can check subject types at compile time.
#[proc_macro]
pub fn model(input: TokenStream) -> TokenStream {
    model_macro::expand(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

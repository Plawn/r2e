//! Proc macros for `r2e-openfga`. See [`model!`](macro@model).
//!
//! The single macro, `model!`, turns a checked-in OpenFGA `.fga` model into a
//! typed authorization API at compile time: the file is parsed and
//! semantically validated (with `r2e-openfga-model`), and a typo in a type or
//! relation name becomes a build error instead of a permanent 403 at runtime.
//!
//! Applications do not depend on this crate: `r2e-openfga` (facade feature
//! `openfga`) re-exports the macro as `r2e_openfga::model!`, and the generated
//! code refers to `r2e_openfga` items (resolved through either `r2e-openfga`
//! or the `r2e` facade).
//!
//! ```rust,ignore
//! r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");
//!
//! #[get("/{doc_id}")]
//! #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
//! async fn view(&self, Path(doc_id): Path<String>) -> Json<Doc> { /* ... */ }
//! ```
//!
//! The generated module exposes `MODEL` (the schema 1.1 JSON, for
//! `OpenFga::model(authz::MODEL)`), a marker type and `id()` constructor per
//! FGA type, a typed relation constant per relation, and `DirectlyAssignable`
//! impls for compile-time checked writes. Guide:
//! [`docs/features/23-openfga.md`](https://github.com/plawn/r2e/blob/master/docs/features/23-openfga.md).

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

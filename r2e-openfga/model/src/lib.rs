//! Parser for the OpenFGA authorization-model DSL (schema 1.1).
//!
//! Turns a `.fga` file into an [`AuthorizationModel`] whose JSON
//! serialization matches the official `openfga/language` transformer
//! byte-for-byte (validated against its vendored DSL↔JSON test corpus).
//! No proc-macro dependencies — usable standalone, from build scripts, or
//! from `r2e-openfga-macros`' `model!`.
//!
//! ```
//! let model = r2e_openfga_model::parse(r#"
//! model
//!   schema 1.1
//!
//! type user
//!
//! type document
//!   relations
//!     define viewer: [user]
//!     define editor: [user] or viewer
//! "#.trim_start()).unwrap();
//!
//! r2e_openfga_model::validate(&model).unwrap();
//! let json = model.to_json();
//! assert_eq!(json["schema_version"], "1.1");
//! ```
//!
//! Two layers, matching the official tooling:
//! - [`parse`] — **syntax only** (structure, operator mixing, duplicates).
//! - [`validate`] — **semantic** referential checks (unknown types,
//!   relations, conditions). The `model!` macro runs both; call `validate`
//!   yourself when using the parser standalone.
//!
//! Conditions are CEL passthrough: parameter lists are typed, the expression
//! body is embedded verbatim in the JSON. Modular models (schema 1.2,
//! `module`/`extend`) are rejected with a clear error.

mod model;
mod parser;
mod validate;

pub use model::{
    AuthorizationModel, Condition, ConditionParamType, Metadata, ObjectRelation, RelationMetadata,
    RelationReference, TypeDefinition, Userset, Wildcard,
};
pub use parser::{parse, ParseError};
pub use validate::{validate, ValidationError};

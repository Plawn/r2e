//! OpenAPI 3.1.0 spec generation for R2E.
//!
//! This crate auto-generates an OpenAPI specification from controller route
//! metadata, with an optional interactive documentation UI at `/docs`.
//!
//! # Dependencies
//!
//! Add both `r2e-openapi` (or `r2e` with `features = ["openapi"]`) **and**
//! `schemars` to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! r2e = { version = "0.1", features = ["openapi"] }
//! schemars = "1"
//! ```
//!
//! `schemars` must be a **direct dependency** because `#[derive(JsonSchema)]`
//! generates code that references the `schemars` crate by name. This is the
//! same pattern as `serde`, `garde`, and other derive-macro crates.
//!
//! # Usage
//!
//! Derive `JsonSchema` on your request/response types:
//!
//! ```ignore
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, JsonSchema)]
//! pub struct CreateUser {
//!     pub name: String,
//!     pub email: String,
//! }
//! ```
//!
//! Then register the plugin:
//!
//! ```ignore
//! use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
//!
//! AppBuilder::new()
//!     .build_state::<AppState, _, _>().await
//!     .with(OpenApiPlugin::new(
//!         OpenApiConfig::new("My API", "1.0.0").with_docs_ui(true),
//!     ))
//!     .register_controller::<UserController>()
//!     .serve("0.0.0.0:3000").await.unwrap();
//! ```

mod builder;
mod ext;
mod handlers;
pub mod schema;

pub use builder::{build_spec, OpenApiConfig};
pub use ext::OpenApiPlugin;
pub use handlers::openapi_routes;
pub use schemars;
pub use schema::{SchemaProvider, SchemaRegistry};

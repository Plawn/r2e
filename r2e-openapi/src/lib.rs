mod builder;
mod ext;
mod handlers;
pub mod schema;

pub use builder::{build_spec, OpenApiConfig};
pub use ext::OpenApiPlugin;
pub use handlers::openapi_routes;
pub use schemars;
pub use schema::{SchemaProvider, SchemaRegistry};

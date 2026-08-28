//! `Params<T>` requires `T: Deserialize + JsonSchema` (the schema becomes the
//! tool's `inputSchema`). A type without the `JsonSchema` derive must fail
//! with a plain trait-bound error at the tool span.

use r2e::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, ObjectParams)]
pub struct NoSchema {
    pub a: f64,
}

#[controller]
pub struct SchemaTools {}

#[mcp_routes]
impl SchemaTools {
    /// Broken: `NoSchema` does not derive `schemars::JsonSchema`.
    #[tool]
    async fn broken(&self, Params(p): Params<NoSchema>) -> String {
        format!("{}", p.a)
    }
}

fn main() {}

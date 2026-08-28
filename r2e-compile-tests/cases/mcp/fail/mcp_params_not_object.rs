//! MCP arguments must have a named-struct root. Scalars cannot derive the
//! sealed `ObjectParams` marker and must fail at the Params type span.

use r2e::prelude::*;

#[controller]
pub struct ScalarTools {}

#[mcp_routes]
impl ScalarTools {
    #[tool]
    async fn broken(&self, Params(value): Params<u32>) -> String {
        value.to_string()
    }
}

fn main() {}

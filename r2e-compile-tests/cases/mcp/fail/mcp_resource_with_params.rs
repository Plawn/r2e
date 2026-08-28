//! A `#[resource]` method cannot take `Params<T>` — `resources/read` carries
//! only a URI, no arguments. Must be rejected with a targeted error.

use r2e::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct Filter {
    pub q: String,
}

#[controller]
pub struct ParamTools {}

#[mcp_routes]
impl ParamTools {
    /// Broken: resources take no arguments.
    #[resource(uri = "r2e://broken")]
    async fn broken(&self, Params(p): Params<Filter>) -> String {
        p.q
    }
}

fn main() {}

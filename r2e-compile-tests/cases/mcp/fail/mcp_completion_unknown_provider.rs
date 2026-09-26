//! `complete(arg = "method")` must name a `#[completion]` method of the impl.

use r2e::prelude::*;

#[controller]
pub struct UnknownProvider {}

#[mcp_routes]
impl UnknownProvider {
    /// Broken.
    #[resource(uri = "files://{name}", complete(name = "file_names"))]
    async fn file(&self, call: ResourceCall) -> String {
        call.uri
    }
}

fn main() {}

//! `complete(...)` on a resource must name a variable of its URI template.

use r2e::prelude::*;

#[controller]
pub struct UnknownVariable {}

#[mcp_routes]
impl UnknownVariable {
    /// Broken.
    #[resource(uri = "files://{name}", complete(path = "paths"))]
    async fn file(&self, call: ResourceCall) -> String {
        call.uri
    }

    #[completion]
    async fn paths(&self, _c: Completion) -> Vec<String> {
        Vec::new()
    }
}

fn main() {}

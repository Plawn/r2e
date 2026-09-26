//! Completion providers answer instantly: no `Progress`, `McpClient` or
//! `McpSession` parameter.

use r2e::prelude::*;

#[controller]
pub struct BadParam {}

#[mcp_routes]
impl BadParam {
    /// Completed.
    #[resource(uri = "files://{name}", complete(name = "names"))]
    async fn file(&self, call: ResourceCall) -> String {
        call.uri
    }

    #[completion]
    async fn names(&self, _c: Completion, progress: Progress) -> Vec<String> {
        let _ = progress;
        Vec::new()
    }
}

fn main() {}

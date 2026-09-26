//! A fixed-URI resource has no variable to complete.

use r2e::prelude::*;

#[controller]
pub struct FixedUri {}

#[mcp_routes]
impl FixedUri {
    /// Broken.
    #[resource(uri = "files://readme", complete(name = "names"))]
    async fn readme(&self) -> String {
        String::new()
    }

    #[completion]
    async fn names(&self, _c: Completion) -> Vec<String> {
        Vec::new()
    }
}

fn main() {}

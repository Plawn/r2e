//! A member takes at most one `McpSession` handle.

use r2e::prelude::*;

#[controller]
pub struct SessionTools {}

#[mcp_routes]
impl SessionTools {
    /// Broken: two session handles.
    #[tool]
    async fn broken(&self, a: McpSession, b: McpSession) -> String {
        let _ = (a, b);
        "hi".to_string()
    }
}

fn main() {}

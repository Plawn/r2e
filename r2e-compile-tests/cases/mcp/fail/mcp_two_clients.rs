//! A member takes at most one `McpClient` — it is a borrow of the call's
//! single back channel.

use r2e::prelude::*;

#[controller]
pub struct TwoClients {}

#[mcp_routes]
impl TwoClients {
    /// Broken.
    #[tool]
    async fn broken(&self, a: McpClient<'_>, b: McpClient<'_>) -> String {
        let _ = (a, b);
        String::new()
    }
}

fn main() {}

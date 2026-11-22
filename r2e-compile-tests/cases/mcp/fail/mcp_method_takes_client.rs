//! An extension method answers its request directly: it cannot take
//! `McpClient` (no elicitation / sampling round-trip).

use r2e::prelude::*;
use r2e::r2e_mcp::McpClient;

#[controller]
pub struct ChattyMethods {}

#[routes]
impl ChattyMethods {
    #[method(name = "acme/ask")]
    async fn ask(&self, _client: McpClient) -> Json<u32> {
        Json(1)
    }
}

fn main() {}

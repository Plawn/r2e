//! An `McpClient` borrows the call: a request to the client is only
//! deliverable while the call is in flight, so it cannot outlive it in a
//! spawned task.

use r2e::prelude::*;

#[controller]
pub struct Escapes {}

#[mcp_routes]
impl Escapes {
    /// Broken.
    #[tool]
    async fn broken(&self, client: McpClient<'_>) -> String {
        r2e::rt::spawn(async move {
            let _ = client.supports_elicitation();
        });
        String::new()
    }
}

fn main() {}

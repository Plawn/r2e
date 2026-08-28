//! `#[resource]` requires a fixed URI — a bare marker must be rejected with
//! an error showing the expected form.

use r2e::prelude::*;

#[controller]
pub struct UrilessTools {}

#[mcp_routes]
impl UrilessTools {
    /// Broken: no `uri`.
    #[resource]
    async fn broken(&self) -> String {
        "contents".to_string()
    }
}

fn main() {}

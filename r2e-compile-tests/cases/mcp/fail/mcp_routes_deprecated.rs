//! `#[mcp_routes]` is a deprecated alias of `#[routes]`: the warning fires at
//! the use site and names the replacement (denied here so trybuild sees it).

#![deny(deprecated)]

use r2e::prelude::*;

#[controller]
pub struct LegacyTools;

#[mcp_routes]
impl LegacyTools {
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

fn main() {}

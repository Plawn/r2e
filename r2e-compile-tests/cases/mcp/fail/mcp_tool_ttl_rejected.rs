//! A tool result has no cache fields: `ttl` is a `#[resource]` key only.

use r2e::prelude::*;

#[controller]
pub struct CachedResources {}

#[routes]
impl CachedResources {
    #[tool(ttl = "30s")]
    async fn broken(&self) -> String {
        "contents".to_string()
    }
}

fn main() {}

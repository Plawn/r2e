//! `#[resource(ttl = ..)]` is a duration, checked at compile time.

use r2e::prelude::*;

#[controller]
pub struct CachedResources {}

#[routes]
impl CachedResources {
    #[resource(uri = "r2e://a", ttl = "soon")]
    async fn broken(&self) -> String {
        "contents".to_string()
    }
}

fn main() {}

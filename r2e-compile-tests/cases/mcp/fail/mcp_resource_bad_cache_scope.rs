//! `#[resource(cache_scope = ..)]` is `"public"` or `"private"`.

use r2e::prelude::*;

#[controller]
pub struct CachedResources {}

#[routes]
impl CachedResources {
    #[resource(uri = "r2e://a", cache_scope = "shared")]
    async fn broken(&self) -> String {
        "contents".to_string()
    }
}

fn main() {}

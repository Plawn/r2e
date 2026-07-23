//! `#[fallback]` cannot be combined with a method attribute — it already
//! matches every HTTP method on every unmatched path.

use r2e::prelude::*;

#[controller]
pub struct MyController {}

#[routes]
impl MyController {
    #[fallback]
    #[get("/x")]
    async fn catch_all(&self) -> &'static str {
        "nope"
    }
}

fn main() {}

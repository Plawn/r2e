//! `#[fallback]` is app-wide (it handles every unmatched request), so it is
//! only allowed on controllers without a path prefix.

use r2e::prelude::*;

#[controller(path = "/api")]
pub struct MyController {}

#[routes]
impl MyController {
    #[fallback]
    async fn catch_all(&self) -> &'static str {
        "nope"
    }
}

fn main() {}

//! `#[fallback]` takes no path argument — it matches whatever no other route
//! matched.

use r2e::prelude::*;

#[controller]
pub struct MyController {}

#[routes]
impl MyController {
    #[fallback("/x")]
    async fn catch_all(&self) -> &'static str {
        "nope"
    }
}

fn main() {}

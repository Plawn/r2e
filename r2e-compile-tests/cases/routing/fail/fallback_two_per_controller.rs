//! A controller may declare at most one `#[fallback]` route — the router has a
//! single fallback slot.

use r2e::prelude::*;

#[controller]
pub struct MyController {}

#[routes]
impl MyController {
    #[fallback]
    async fn first(&self) -> &'static str {
        "first"
    }

    #[fallback]
    async fn second(&self) -> &'static str {
        "second"
    }
}

fn main() {}

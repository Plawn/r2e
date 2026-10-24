//! The `#[layer]` closure form receives exactly one argument: the controller
//! core.

use r2e::prelude::*;

#[controller]
pub struct MyController {}

#[routes]
impl MyController {
    #[get("/")]
    #[layer(|this: &Self, n: usize| r2e::http::DefaultBodyLimit::max(n))]
    async fn index(&self) -> &'static str {
        "ok"
    }
}

fn main() {}

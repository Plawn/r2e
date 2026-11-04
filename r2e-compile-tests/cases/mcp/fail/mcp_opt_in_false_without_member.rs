//! Every `#[routes(...)]` argument is MCP-only: a parsed-away `opt_in = false`
//! on an impl without MCP members is still rejected, not silently ignored.

use r2e::prelude::*;

#[controller(path = "/calc")]
pub struct Calc;

#[routes(opt_in = false)]
impl Calc {
    #[get("/add")]
    async fn add(&self) -> String {
        "3".to_string()
    }
}

#[controller]
pub struct Empty;

#[routes(opt_in = false)]
impl Empty {}

fn main() {}

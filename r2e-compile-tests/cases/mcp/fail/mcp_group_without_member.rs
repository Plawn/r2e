//! `group`/`opt_in` configure the MCP members of a `#[routes]` impl: on an
//! impl with HTTP routes only they would do nothing.

use r2e::prelude::*;

#[controller(path = "/calc")]
pub struct Calc;

#[routes(group = "x")]
impl Calc {
    #[get("/add")]
    async fn add(&self) -> String {
        "3".to_string()
    }
}

fn main() {}

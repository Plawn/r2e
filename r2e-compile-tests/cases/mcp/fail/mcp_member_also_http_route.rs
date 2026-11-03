//! One method is one member: an HTTP route and an MCP tool are separate
//! methods of a `#[routes]` impl. The error lands on the second marker and
//! names both.

use r2e::prelude::*;

#[controller(path = "/calc")]
pub struct Calc;

#[routes]
impl Calc {
    #[get("/add")]
    #[tool]
    async fn add(&self) -> String {
        "3".to_string()
    }
}

fn main() {}

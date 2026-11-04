//! One type exposes one port: MCP members cannot share a `#[routes]` impl
//! with HTTP routes. The error lands on the first MCP marker and points at
//! the two-types shape.

use r2e::prelude::*;

#[controller(path = "/calc")]
pub struct Calc;

#[routes]
impl Calc {
    #[get("/add")]
    async fn add_route(&self) -> String {
        "3".to_string()
    }

    #[tool]
    async fn add(&self) -> String {
        "3".to_string()
    }
}

fn main() {}

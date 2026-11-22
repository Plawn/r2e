//! Two `#[method]` members of one impl cannot answer the same JSON-RPC
//! method.

use r2e::prelude::*;

#[controller]
pub struct TwiceMethods {}

#[routes]
impl TwiceMethods {
    #[method(name = "acme/list")]
    async fn first(&self) -> Json<u32> {
        Json(1)
    }

    #[method(name = "acme/list")]
    async fn second(&self) -> Json<u32> {
        Json(2)
    }
}

fn main() {}

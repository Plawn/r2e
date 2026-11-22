//! One type = one port: a `#[method]` cannot share an impl with HTTP routes.

use r2e::prelude::*;

#[controller(path = "/mixed")]
pub struct MixedMethods {}

#[routes]
impl MixedMethods {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "ok"
    }

    #[method(name = "acme/list")]
    async fn list(&self) -> Json<u32> {
        Json(1)
    }
}

fn main() {}

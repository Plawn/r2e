//! `#[method]` takes `name`, `scopes` and `any_scopes` only — a method is
//! not listed, so it has no `group`.

use r2e::prelude::*;

#[controller]
pub struct GroupedMethods {}

#[routes]
impl GroupedMethods {
    #[method(name = "acme/list", group = "admin")]
    async fn list(&self) -> Json<u32> {
        Json(1)
    }
}

fn main() {}

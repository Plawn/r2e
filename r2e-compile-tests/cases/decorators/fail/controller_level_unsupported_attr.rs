//! A route-family attribute with no controller-level meaning on a `#[routes]`
//! impl block must be a targeted compile error, never a silent no-op.

use r2e::prelude::*;

#[controller(path = "/c")]
pub struct Ctrl {}

#[routes]
#[anonymous]
impl Ctrl {
    #[get("/a")]
    async fn a(&self) -> String {
        "a".into()
    }
}

fn main() {}

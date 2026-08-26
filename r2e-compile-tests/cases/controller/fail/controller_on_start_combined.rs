//! `#[on_start]` is a plain startup observer — it cannot double as a route.

use r2e::prelude::*;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[get("/")]
    #[on_start]
    async fn warm(&self) -> &'static str {
        "x"
    }
}

fn main() {}

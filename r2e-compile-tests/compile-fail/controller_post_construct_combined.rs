//! `#[post_construct]` is a plain lifecycle hook — it cannot double as a route.

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[get("/")]
    #[post_construct]
    async fn init(&self) -> &'static str {
        "x"
    }
}

fn main() {}

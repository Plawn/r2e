//! `#[pre_destroy]` is a plain disposal hook — it cannot double as a route.

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[get("/")]
    #[pre_destroy]
    async fn shutdown(&self) -> &'static str {
        "x"
    }
}

fn main() {}

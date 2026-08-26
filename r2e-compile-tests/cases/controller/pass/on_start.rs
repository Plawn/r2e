//! `#[on_start]` compiles on both `#[bean]` impls and `#[routes]` controller
//! impls (parity with `#[post_construct]` / `#[pre_destroy]`).

use r2e::prelude::*;

#[derive(Clone)]
pub struct Warmer;

#[bean]
impl Warmer {
    pub fn new() -> Self {
        Self
    }

    #[on_start(order = -1)]
    async fn warm(&self) {}
}

#[controller(path = "/svc")]
pub struct Svc {
    #[inject]
    _warmer: Warmer,
}

#[routes]
impl Svc {
    #[get("/")]
    async fn root(&self) -> &'static str {
        "ok"
    }

    #[on_start]
    async fn warm(&self) {}

    #[on_start(order = 5)]
    fn warm_late(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

fn main() {}

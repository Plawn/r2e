//! `#[cfg_attr]` on a request-scoped controller field is rejected, and the
//! message names `#[cfg_attr]` rather than `#[cfg]`.
//!
//! The diagnostic is *deferred*: `#[controller]` still emits the whole
//! expansion (meta module, façade, `ContextConstruct`), so `#[routes]` has
//! nothing extra to complain about and this file produces exactly ONE error
//! instead of burying it under two unrelated trait errors (task #985).
use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[controller(path = "/orders")]
pub struct OrderController {
    #[inject]
    svc: Svc,
    #[cfg_attr(all(), derive(Clone))]
    #[inject(request)]
    correlation: String,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}

//! `#[cfg]` on a request-scoped controller field cannot be honoured — the
//! generated request extractor's marker tuple is positional — so it is a hard
//! error rather than a silent no-op that leaves the extraction running
//! (task #985).
use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[controller(path = "/orders")]
pub struct OrderController {
    #[inject]
    svc: Svc,
    #[cfg(any())]
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

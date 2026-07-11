//! `#[middleware]`, `#[layer]`, and `#[pre_guard]` attach to a
//! `.route(path, method_router)` registration; a fallback is registered via
//! `Router::fallback(handler)`, which takes no layers.

use r2e::prelude::*;

async fn noop_mw(
    req: r2e::http::extract::Request,
    next: r2e::http::middleware::Next,
) -> r2e::http::response::Response {
    next.run(req).await
}

#[controller]
pub struct MyController {}

#[routes]
impl MyController {
    #[fallback]
    #[middleware(noop_mw)]
    async fn catch_all(&self) -> &'static str {
        "nope"
    }
}

fn main() {}

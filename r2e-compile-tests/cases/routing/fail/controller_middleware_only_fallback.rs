//! Controller-level `#[middleware]`/`#[layer]` wrap every `.route(...)`
//! registration; a `#[fallback]` is skipped (`Router::fallback` takes no
//! per-route layers). With only a fallback, they would apply to nothing.

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
#[middleware(noop_mw)]
impl MyController {
    #[fallback]
    async fn catch_all(&self) -> &'static str {
        "nope"
    }
}

fn main() {}

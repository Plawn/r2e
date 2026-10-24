//! `#[middleware(fn, ..)]` accepts the `state` flag only.

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
    #[get("/")]
    #[middleware(noop_mw, stateful)]
    async fn index(&self) -> &'static str {
        "ok"
    }
}

fn main() {}

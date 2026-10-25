//! Controller-level `#[middleware]`/`#[layer]`, the `state` middleware form
//! and the `|this: &Self|` layer closure, at both levels.

use std::sync::Arc;

use r2e::http::extract::Request;
use r2e::http::middleware::Next;
use r2e::http::response::Response;
use r2e::http::DefaultBodyLimit;
use r2e::prelude::*;

async fn plain_mw(req: Request, next: Next) -> Response {
    next.run(req).await
}

async fn stateful_mw(
    ctrl: Arc<ProxyController>,
    req: Request,
    next: Next,
) -> Response {
    let _limit = ctrl.body_limit;
    next.run(req).await
}

#[controller]
pub struct ProxyController {
    #[config("proxy.request-body-limit-bytes")]
    body_limit: usize,
}

#[routes]
#[middleware(plain_mw)]
#[middleware(stateful_mw, state)]
#[layer(|this: &Self| DefaultBodyLimit::max(this.body_limit))]
#[layer(DefaultBodyLimit::max(1 << 20))]
impl ProxyController {
    #[post("/a")]
    async fn a(&self, body: String) -> String {
        body
    }

    #[post("/b")]
    #[middleware(stateful_mw, state)]
    #[layer(|this| DefaultBodyLimit::max(this.body_limit / 2))]
    async fn b(&self, body: String) -> String {
        body
    }

    #[fallback]
    async fn rest(&self) -> &'static str {
        "fallback"
    }
}

fn main() {}

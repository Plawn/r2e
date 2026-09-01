//! Scaffolding for `llm/proxy-catch-all.md`.

use r2e::http::extract::Request;
use r2e::prelude::*;

/// The upstream the proxy controller forwards to.
#[derive(Clone)]
pub struct UpstreamClient;

impl UpstreamClient {
    /// Streams the request through and hands back the upstream's response.
    pub async fn forward(&self, req: Request) -> Response {
        Response::new(Body::new(req.into_body()))
    }
}

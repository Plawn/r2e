use r2e::prelude::*;

async fn noop_mw(
    req: r2e::http::extract::Request,
    next: r2e::http::middleware::Next,
) -> r2e::http::response::Response {
    next.run(req).await
}

#[controller]
pub struct Tools;

// MCP tools sit behind one shared transport endpoint: no per-tool (or
// per-impl) tower stack.
#[mcp_routes]
#[middleware(noop_mw)]
impl Tools {
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

fn main() {}

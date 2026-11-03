//! Impl-level `#[middleware]` is HTTP-only: on a `#[routes]` impl that has an
//! HTTP route it wraps the HTTP routes and is never applied to the MCP
//! members (no "not supported" error as on an MCP-only impl).

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;

async fn noop_mw(
    req: r2e::http::extract::Request,
    next: r2e::http::middleware::Next,
) -> r2e::http::response::Response {
    next.run(req).await
}

#[controller(path = "/tools")]
pub struct Tools;

#[routes]
#[middleware(noop_mw)]
impl Tools {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[tool]
    async fn ping_tool(&self) -> String {
        "pong".to_string()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .plugin(r2e::r2e_mcp::McpServer::new())
            .build_state()
            .await
            .register_controller::<Tools>()
            .register_mcp_service::<Tools>()
    };
}

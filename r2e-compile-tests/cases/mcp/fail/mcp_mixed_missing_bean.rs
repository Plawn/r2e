//! A mixed `#[routes]` impl has ONE `EndpointDeps` carrier folding the HTTP
//! and MCP sites: a bean missing from the state is reported at
//! `register_controller()` AND at `register_mcp_service()`.

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;

#[derive(Clone)]
pub struct CalcService;

#[controller(path = "/calc")]
pub struct Calc {
    #[inject]
    calc: CalcService,
}

#[routes]
impl Calc {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        let _ = &self.calc;
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
            .build_state()
            .await
            .register_controller::<Calc>()
    };
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_mcp_service::<Calc>()
    };
}

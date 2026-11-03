//! A mixed `#[routes]` impl's impl-level `#[guard(...)]` applies to its HTTP
//! routes AND its MCP members, and its spec's deps are folded into the type's
//! ONE `EndpointDeps` carrier: a bean only the guard reads, missing from the
//! state, is reported at `register_controller()` AND at
//! `register_mcp_service()` — not just at whichever transport built the guard
//! first. (The core itself injects nothing.)

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;
use std::future::Future;

/// The bean the guard needs — deliberately never provided.
#[derive(Clone)]
pub struct Policy;

#[derive(DecoratorBean)]
pub struct PolicyGuard {
    #[inject]
    policy: Policy,
}

impl<I: Identity> Guard<I> for PolicyGuard {
    fn check(&self, _ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Rejection>> + Send {
        let _ = &self.policy;
        async { Ok(()) }
    }
}

#[controller(path = "/calc")]
pub struct Calc;

#[routes]
#[guard(PolicyGuard::spec())]
impl Calc {
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

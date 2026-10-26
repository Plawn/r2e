//! An `#[inject(identity)] AuthenticatedUser` parameter on an MCP tool
//! extracts through the same bean-backed extractor an HTTP route uses, which
//! needs the `Arc<JwtClaimsValidator>` bean. Registering the service on an
//! app that never provided it must fail at `register_mcp_service()` — not
//! at the first call.

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;

#[controller]
pub struct WhoAmITools {}

#[mcp_routes]
impl WhoAmITools {
    /// Who am I?
    #[tool]
    async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        user.sub
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_mcp_service::<WhoAmITools>()
    };
}

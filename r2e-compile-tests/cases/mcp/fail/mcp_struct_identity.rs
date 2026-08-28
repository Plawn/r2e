//! MCP identity is method-scoped; a struct-level request identity belongs to
//! HTTP controller facades and must produce a targeted diagnostic.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[controller]
pub struct StructIdentityTools {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[mcp_routes]
impl StructIdentityTools {
    #[tool]
    async fn broken(&self) -> String {
        "broken".to_string()
    }
}

fn main() {}

//! A struct-level request scope on an `#[mcp_routes]` service: the same
//! `#[inject(identity)]` / `#[inject(request)]` fields an HTTP controller
//! declares, extracted once per HTTP request on the MCP endpoint. Members
//! read them as `self.<field>` on the request façade; `#[request_helper]`
//! helpers live there too; `#[roles]` gates on the struct identity.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::http::header::HeaderMap;

#[controller]
pub struct StructIdentityTools {
    #[inject(identity)]
    user: AuthenticatedUser,
    #[inject(request)]
    headers: HeaderMap,
}

#[mcp_routes]
impl StructIdentityTools {
    #[tool]
    async fn whoami(&self) -> String {
        format!("{} ({} headers)", self.user.sub, self.headers.len())
    }

    #[tool]
    #[roles("admin")]
    async fn admin_only(&self) -> String {
        self.greeting()
    }

    #[resource(uri = "r2e://me")]
    async fn me(&self) -> String {
        self.user.sub.clone()
    }

    #[prompt]
    async fn hello(&self) -> String {
        format!("hello {}", self.user.sub)
    }

    #[request_helper]
    fn greeting(&self) -> String {
        format!("welcome {}", self.user.sub)
    }
}

#[controller]
pub struct OptionalIdentityTools {
    #[inject(identity)]
    user: Option<AuthenticatedUser>,
}

#[mcp_routes]
impl OptionalIdentityTools {
    #[tool]
    async fn maybe(&self) -> String {
        self.user.as_ref().map_or("anonymous", |u| u.sub.as_str()).to_string()
    }
}

fn main() {
    // Registration infers the witness (request-data marker + per-member
    // markers) and checks the identity's bean dependency, like HTTP routes.
    let _ = async {
        AppBuilder::new()
            .provide(std::sync::Arc::new(r2e_test::TestJwt::new().claims_validator()))
            .plugin(r2e::r2e_mcp::McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<StructIdentityTools>()
            .register_mcp_service::<OptionalIdentityTools>()
    };
}

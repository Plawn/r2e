//! Decorator expressions on an `#[routes]` impl are evaluated with the
//! controller in scope: `Self::…` works at both the impl level and the member
//! level, because the prebuilt sets are constructed by associated functions
//! of the controller.

use r2e::prelude::*;
use r2e::{GuardContext, Identity};
use std::future::Future;

pub struct AllowAll;

impl SelfBuilt for AllowAll {}

impl<I: Identity> Guard<I> for AllowAll {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        async { Ok(()) }
    }
}

#[controller]
pub struct SelfScopedTools {}

impl SelfScopedTools {
    fn make_guard() -> AllowAll {
        AllowAll
    }
}

#[routes]
#[guard(AllowAll = Self::make_guard())]
impl SelfScopedTools {
    #[tool]
    #[guard(AllowAll = Self::make_guard())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .plugin(r2e::r2e_mcp::McpServer::new())
            .build_state()
            .await
            .register_mcp_service::<SelfScopedTools>()
    };
}

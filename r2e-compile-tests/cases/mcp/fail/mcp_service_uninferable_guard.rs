//! An uninferable impl-level `#[guard(...)]` on an MCP service is reported,
//! never silently dropped.
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

fn make_guard() -> AllowAll {
    AllowAll
}

#[controller]
pub struct EmptyTools;

#[routes]
#[guard(make_guard())]
impl EmptyTools {
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

fn main() {}

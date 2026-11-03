//! An impl-level `#[guard(...)]` whose spec type cannot be inferred is
//! reported even when the `#[mcp_routes]` service has no member to carry it —
//! the site is never silently dropped.

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
pub struct EmptyTools {}

#[mcp_routes]
#[guard(make_guard())]
impl EmptyTools {}

fn main() {}

//! An uninferable impl-level `#[guard(...)]` is reported even on an MCP
//! service with no member (only the deprecated `#[mcp_routes]` alias spells an
//! empty one; `#[routes]` reads it as HTTP): never silently dropped.
#![allow(deprecated)]
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

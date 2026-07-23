//! A `#[guard(...)]` expression whose spec type cannot be inferred from its
//! leading type path (a free function call) must point at the explicit
//! `SpecType = expr` escape hatch.

use r2e::prelude::*;
use r2e::{GuardContext, Identity};
use std::future::Future;

pub struct AllowAll;

impl SelfBuilt for AllowAll {}

impl<I: Identity> Guard<I> for AllowAll {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

fn make_guard() -> AllowAll {
    AllowAll
}

#[controller(path = "/g")]
pub struct GuardedController {}

#[routes]
impl GuardedController {
    #[get("/")]
    #[guard(make_guard())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {}

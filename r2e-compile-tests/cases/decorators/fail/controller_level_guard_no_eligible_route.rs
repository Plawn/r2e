//! A controller-level `#[guard]` that applies to no route (every route is
//! `#[anonymous]`) is dead configuration — reject it instead of building a
//! guard that never runs.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::{Guard, GuardContext, Identity};
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

#[controller(path = "/c")]
pub struct Ctrl {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
#[guard(AllowAll)]
impl Ctrl {
    #[get("/open")]
    #[anonymous]
    async fn open(&self) -> String {
        "open".into()
    }
}

fn main() {}

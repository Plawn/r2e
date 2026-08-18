//! A decorator attribute placed ABOVE `#[routes]` expands first (attribute
//! macros expand top-down), so `#[routes]` never sees it — before task #906
//! it was silently dropped. It must now be a targeted compile error telling
//! the user to move it below the transforming macro.

use r2e::prelude::*;
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
pub struct Ctrl {}

#[guard(AllowAll)]
#[routes]
impl Ctrl {
    #[get("/a")]
    async fn a(&self) -> String {
        "a".into()
    }
}

fn main() {}

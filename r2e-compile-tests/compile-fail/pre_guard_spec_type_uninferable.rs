//! A `#[pre_guard(...)]` expression whose spec type cannot be inferred must
//! produce the escape-hatch error only — the route registration degrades to
//! the no-pre-guard shape (no missing-constructor cascade).

use r2e::prelude::*;
use r2e::{PreAuthGuardContext};
use std::future::Future;

pub struct AllowAllPre;

impl SelfBuilt for AllowAllPre {}

impl PreAuthGuard for AllowAllPre {
    fn check(
        &self,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

fn make_pre_guard() -> AllowAllPre {
    AllowAllPre
}

#[controller(path = "/g")]
pub struct GatedController {}

#[routes]
impl GatedController {
    #[get("/")]
    #[pre_guard(make_pre_guard())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {}

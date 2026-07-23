//! An impl-level `#[intercept]` on a `#[bean]` impl with no `#[scheduled]` or
//! `#[consumer]` method applies to nothing — it is a fail-loud compile error
//! rather than a silent no-op.

use r2e::prelude::*;
use r2e::r2e_utils::Logged;

#[bean]
#[derive(Clone)]
pub struct CleanupService {}

#[bean]
#[intercept(Logged::info())]
impl CleanupService {
    pub fn new() -> Self {
        Self {}
    }

    // A plain helper — neither #[scheduled] nor #[consumer].
    async fn helper(&self) {}
}

fn main() {}

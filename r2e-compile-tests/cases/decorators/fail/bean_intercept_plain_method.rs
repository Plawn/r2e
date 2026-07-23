//! `#[intercept]` on a bean method that is neither `#[scheduled]` nor
//! `#[consumer]` — there is no dispatch wrapper to run the chain, so it is
//! rejected.

use r2e::prelude::*;
use r2e::r2e_utils::Logged;

#[bean]
#[derive(Clone)]
pub struct CleanupService {}

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self {}
    }

    #[intercept(Logged::info())]
    async fn helper(&self) {}
}

fn main() {}

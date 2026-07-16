//! A struct literal of an intercepted bean written OUTSIDE the `#[bean]` impl
//! block is not rewritten to initialize the hidden decorator slot, so it fails
//! with E0063 `missing field __r2e_decos`. The field is `pub #[doc(hidden)]`,
//! so such code CAN initialize it explicitly as an escape hatch — this fixture
//! documents the failure when it does not.

use r2e::prelude::*;
use r2e::r2e_utils::Logged;

#[bean]
#[derive(Clone)]
pub struct CleanupService {
    tag: &'static str,
}

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self { tag: "in-block" } // rewritten by the impl macro — OK
    }

    #[scheduled(every = 10)]
    #[intercept(Logged::info())]
    async fn purge(&self) {}
}

// Outside the impl block: NOT rewritten → missing `__r2e_decos`.
fn make() -> CleanupService {
    CleanupService { tag: "outside" }
}

fn main() {
    let _ = make();
}

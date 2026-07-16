//! `#[intercept]` in a `#[bean]` impl whose struct was NOT annotated with the
//! `#[bean]` struct attribute: the hidden decorator slot is missing, so the
//! generated wrapper's `<Self as HasDecoSlot>` fails with the guidance
//! diagnostic (plus an unavoidable secondary error from the constructor
//! literal, which the struct attribute would have supplied the field for).

use r2e::prelude::*;
use r2e::r2e_utils::Logged;

// NOTE: no `#[bean]` on the struct — only on the impl.
#[derive(Clone)]
pub struct CleanupService {}

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self {}
    }

    #[scheduled(every = 10)]
    #[intercept(Logged::info())]
    async fn purge(&self) {}
}

fn main() {}

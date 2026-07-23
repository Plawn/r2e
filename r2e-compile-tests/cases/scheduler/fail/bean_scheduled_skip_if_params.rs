//! A `skip_if` predicate is called without arguments on every tick, so it
//! must take only `&self`.

use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupBean;

#[bean]
impl CleanupBean {
    pub fn new() -> Self {
        Self
    }

    fn paused(&self, _region: &str) -> bool {
        false
    }

    #[scheduled(every = "50ms", skip_if = "paused")]
    async fn tick(&self) {}
}

fn main() {}

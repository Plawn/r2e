//! Bean path: `schedule = "..."` must name a plain sync method among the
//! bean's plain methods (a `#[scheduled]`/lifecycle method does not count).

use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupBean;

#[bean]
impl CleanupBean {
    pub fn new() -> Self {
        Self
    }

    #[scheduled(schedule = "cleanup_cadence")]
    async fn tick(&self) {}
}

fn main() {}

//! `#[async_exec]` on a `&mut self` bean method — rejected: the generated
//! wrapper clones an immutable handle to submit the body to the pool, so it
//! cannot forward a mutable receiver.

use r2e::prelude::*;
use r2e::r2e_executor::PoolExecutor;

#[derive(Clone)]
pub struct ReportService {
    executor: PoolExecutor,
}

#[bean]
impl ReportService {
    pub fn new(executor: PoolExecutor) -> Self {
        Self { executor }
    }

    #[async_exec]
    async fn generate_pdf(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

fn main() {}

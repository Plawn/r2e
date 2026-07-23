//! `#[async_exec]` + `#[scheduled]` on one bean method — rejected: the
//! pool-submission rewrite and the scheduled wiring are mutually exclusive.

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
    #[scheduled(every = "5m")]
    async fn generate_pdf(&self) {}
}

fn main() {}

//! `#[intercept]` on an `#[async_exec]` bean method — rejected: the
//! pool-submission wrapper does not run an interceptor chain.

use r2e::prelude::*;
use r2e::r2e_executor::PoolExecutor;
use r2e::r2e_utils::Logged;

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
    #[intercept(Logged::info())]
    async fn generate_pdf(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn main() {}

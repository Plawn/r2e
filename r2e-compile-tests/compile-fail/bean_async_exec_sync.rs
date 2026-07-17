//! `#[async_exec]` on a sync bean method — rejected: the body is submitted
//! to a `PoolExecutor`, so it must be an `async fn`.

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
    fn generate_pdf(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn main() {}

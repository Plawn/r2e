//! `#[async_exec]` + `#[scheduled]` on one controller method — rejected (same
//! diagnostic as the bean path): the pool-submission rewrite and the scheduled
//! wiring are mutually exclusive. Without the shared pre-check the scheduled
//! branch would classify the method first and silently drop the async_exec
//! rewrite.

use r2e::prelude::*;
use r2e::r2e_executor::PoolExecutor;

#[controller(path = "/reports")]
#[derive(Clone)]
pub struct ReportController {
    #[inject]
    executor: PoolExecutor,
}

#[routes]
impl ReportController {
    #[async_exec]
    #[scheduled(every = "5m")]
    async fn generate_pdf(&self) {}
}

fn main() {}

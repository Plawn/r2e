//! `#[intercept]` on a controller `#[async_exec]` method — rejected (same
//! diagnostic as the bean path): the pool-submission wrapper does not run an
//! interceptor chain, so the interceptor would be silently dropped.

use r2e::prelude::*;
use r2e::r2e_executor::PoolExecutor;
use r2e::r2e_utils::Logged;

#[controller(path = "/reports")]
#[derive(Clone)]
pub struct ReportController {
    #[inject]
    executor: PoolExecutor,
}

#[routes]
impl ReportController {
    #[async_exec]
    #[intercept(Logged::info())]
    async fn generate_pdf(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn main() {}

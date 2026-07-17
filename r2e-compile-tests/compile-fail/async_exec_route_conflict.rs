//! `#[get(...)]` + `#[async_exec]` on one controller method — rejected: the
//! pool-submission rewrite and a route registration are mutually exclusive.
//! Without the shared pre-check the route branch would classify the method
//! first and the async_exec rewrite would 404 (never registered as a route).

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
    #[get("/pdf")]
    #[async_exec]
    async fn generate_pdf(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn main() {}

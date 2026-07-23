//! `#[async_exec]` on `#[bean]` — the method is rewritten into a
//! pool-submission wrapper returning `Result<JobHandle<T>, RejectedError>`.
//! Pure per-method codegen: no registration hook, composes with a plain
//! `.register::<T>()`.

use r2e::prelude::*;
use r2e::r2e_executor::{JobHandle, PoolExecutor, RejectedError};

#[derive(Clone)]
pub struct ReportService {
    executor: PoolExecutor,
    io_pool: PoolExecutor,
}

#[bean]
impl ReportService {
    pub fn new(executor: PoolExecutor) -> Self {
        let io_pool = executor.clone();
        Self { executor, io_pool }
    }

    #[async_exec]
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        format!("PDF #{id}").into_bytes()
    }

    #[async_exec(executor = "io_pool")]
    async fn export(&self) -> usize {
        42
    }

    // Destructuring param patterns stay on the inner fn; the wrapper re-binds
    // the value as a plain ident and forwards it.
    #[async_exec]
    async fn sum(&self, (a, b): (u32, u32)) -> u32 {
        a + b
    }
}

fn assert_wrapper_signatures(svc: &ReportService) {
    let _pdf: Result<JobHandle<Vec<u8>>, RejectedError> = svc.generate_pdf(7);
    let _export: Result<JobHandle<usize>, RejectedError> = svc.export();
    let _sum: Result<JobHandle<u32>, RejectedError> = svc.sum((1, 2));
}

fn main() {
    let _ = assert_wrapper_signatures;
}

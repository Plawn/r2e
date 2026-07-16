use r2e::prelude::*;
use r2e::r2e_utils::Logged;

#[derive(Clone)]
pub struct CleanupService;

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self
    }

    #[scheduled(every = 10)]
    #[intercept(Logged::info())]
    async fn purge(&self) {}
}

fn main() {}

use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupService;

#[bean(lazy)]
impl CleanupService {
    pub fn new() -> Self {
        Self
    }

    #[scheduled(every = 10)]
    async fn purge(&self) {}
}

fn main() {}

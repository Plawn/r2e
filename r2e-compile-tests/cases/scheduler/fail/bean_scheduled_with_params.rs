use r2e::prelude::*;

#[derive(Clone)]
pub struct CleanupService;

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self
    }

    #[scheduled(every = 10)]
    async fn purge(&self, name: String) {
        println!("{}", name);
    }
}

fn main() {}

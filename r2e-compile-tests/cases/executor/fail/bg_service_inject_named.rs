//! `#[inject(name = "...")]` on a `#[derive(BackgroundService)]` field used to
//! be silently ignored; it is now the shared named-bean rejection.
use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool;

#[derive(BackgroundService)]
pub struct Worker {
    #[inject(name = "primary")]
    pool: DbPool,
}

impl Worker {
    async fn run(&self) {
        let _ = &self.pool;
    }
}

fn main() {}

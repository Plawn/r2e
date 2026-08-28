//! `#[inject(name = "...")]` was removed on `#[bean]` constructor parameters.
use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool;

#[derive(Clone)]
pub struct Repo {
    _pool: DbPool,
}

#[bean]
impl Repo {
    fn new(#[inject(name = "primary")] pool: DbPool) -> Self {
        Self { _pool: pool }
    }
}

fn main() {}

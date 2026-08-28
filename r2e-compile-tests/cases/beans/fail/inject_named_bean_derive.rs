//! `#[inject(name = "...")]` was removed on `#[derive(Bean)]` fields.
use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool;

#[derive(Clone, Bean)]
pub struct Repo {
    #[inject(name = "primary")]
    pool: DbPool,
}

fn main() {}

//! `#[inject(name = "...")]` was removed on `#[controller]` fields — the same
//! shared diagnostic as every other host.
use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool;

#[controller(path = "/x")]
pub struct XController {
    #[inject(name = "primary")]
    pool: DbPool,
}

fn main() {}

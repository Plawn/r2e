//! `#[producer(name = "...")]` was removed: R2E has no bean qualifiers.
//! Declare a newtype (`struct PrimaryPool(DbPool)`) and produce that instead.
use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool {
    _url: String,
}

#[producer(name = "primary")]
fn create_pool() -> DbPool {
    DbPool {
        _url: String::new(),
    }
}

fn main() {}

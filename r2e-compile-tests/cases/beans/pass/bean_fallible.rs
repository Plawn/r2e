//! Fallible construction: `#[bean]` accepts `-> Result<Self, E>` (sync and
//! async) and `#[producer]` accepts `-> Result<T, E>`, with the error type
//! staying out of the registered bean type — consumers inject `Pool`, never
//! `Result<Pool, _>`.

use r2e::prelude::*;

#[derive(Debug)]
pub struct ConnectFailed;

impl std::fmt::Display for ConnectFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not connect")
    }
}

impl std::error::Error for ConnectFailed {}

#[derive(Clone)]
pub struct Pool;

#[producer]
async fn connect_pool() -> Result<Pool, ConnectFailed> {
    Ok(Pool)
}

#[derive(Clone)]
pub struct Guard;

#[bean]
impl Guard {
    fn new() -> Result<Self, ConnectFailed> {
        Ok(Self)
    }
}

#[derive(Clone)]
pub struct AsyncGuard;

#[bean]
impl AsyncGuard {
    async fn new() -> Result<Self, ConnectFailed> {
        Ok(Self)
    }
}

/// The consumer injects the produced type itself.
#[derive(Clone)]
pub struct Repo {
    #[allow(dead_code)]
    pool: Pool,
}

#[bean]
impl Repo {
    fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

fn main() {}

//! `#[bean]` on an impl whose constructor is an `unsafe fn` is a compile error,
//! for the same reason as `#[producer]`: the generated `Bean::build` /
//! `AsyncBean::build` is safe and is the only caller (task #985).
use r2e::prelude::*;

#[derive(Clone)]
pub struct SyncService;

#[bean]
impl SyncService {
    unsafe fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct AsyncService;

#[bean]
impl AsyncService {
    async unsafe fn new() -> Result<Self, std::io::Error> {
        Ok(Self)
    }
}

fn main() {}

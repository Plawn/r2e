//! An aggregate owns no controllers — each member module carries its own
//! `prefix`. A prefix on the aggregate has nothing to mount, so it is
//! rejected like every other key mixed with `modules(...)`.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

#[module(providers(Svc))]
pub struct RealModule;

#[module(modules(RealModule), prefix = "/api/v1")]
pub struct BadAggregate;

fn main() {}

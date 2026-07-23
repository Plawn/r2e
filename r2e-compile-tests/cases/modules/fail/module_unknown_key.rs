//! `#[module]` must reject unknown declaration keys with a helpful message.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

#[module(provider(Svc))]
pub struct BadModule;

fn main() {}

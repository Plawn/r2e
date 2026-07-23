//! `module(...)` is only valid inside `imports(...)`; using it in any other
//! key (here `providers`) must be a targeted macro error.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

#[module(providers(module(Svc)))]
pub struct BadModule;

fn main() {}

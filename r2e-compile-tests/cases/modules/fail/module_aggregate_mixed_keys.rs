//! `modules(...)` declares an **aggregate** — a named list of modules to
//! register — so it composes nothing else: mixing it with `providers(...)`,
//! `controllers(...)` or any other key would silently drop one of the two
//! meanings. It must be rejected at the declaration.

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

#[module(modules(RealModule), providers(Svc))]
pub struct BadAggregate;

fn main() {}

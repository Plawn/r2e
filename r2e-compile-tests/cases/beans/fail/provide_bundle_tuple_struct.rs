//! `#[derive(ProvideBundle)]` needs named fields — the generated chain reads
//! `bundle.field` per provision.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Pool;

#[derive(ProvideBundle)]
pub struct Env(pub Pool);

fn main() {}

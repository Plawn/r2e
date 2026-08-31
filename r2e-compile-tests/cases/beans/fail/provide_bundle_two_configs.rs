//! A `ProvideBundle` carries at most one `R2eConfig` field: it becomes
//! `override_config(..)`, and a second one would silently discard the first.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Pool;

#[derive(ProvideBundle)]
pub struct Env {
    pub config: R2eConfig,
    pub other_config: R2eConfig,
    pub pool: Pool,
}

fn main() {}
